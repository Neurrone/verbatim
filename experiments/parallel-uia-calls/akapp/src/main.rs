//! akapp: a minimal AccessKit Windows provider for the parallel-uia-calls
//! experiment. One hidden top-level window per UI thread (`threads=<n>`),
//! each with a root and 8 buttons. Clicking (UIA Invoke) button 1 runs the
//! app's action handler, which sleeps for the delay WM_APP+4 sets; every
//! action counts how many are inside it at once and how many run off the
//! window's own thread. WM_APP+1 (posted, wParam ms) keeps the UI thread
//! busy without pumping; WM_APP+2 returns and resets the counters
//! (peak | off << 16 | calls << 40), as the synthetic provider does.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::SeqCst};

use accesskit::{Action, ActionHandler, ActionRequest, ActivationHandler, Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate};
use accesskit_windows::Adapter;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

static DELAY: AtomicU64 = AtomicU64::new(0);
static CUR: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
static OFF: AtomicU64 = AtomicU64::new(0);
static UI_TIDS: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];

const ROOT: NodeId = NodeId(0);

fn tree(index: usize) -> TreeUpdate {
    let mut root = Node::new(Role::Window);
    root.set_label(format!("puia-ak-{index}"));
    root.set_children((1..=8).map(NodeId).collect::<Vec<_>>());
    let mut nodes = vec![(ROOT, root)];
    for i in 1..=8 {
        let mut b = Node::new(Role::Button);
        b.set_label(format!("button {i}"));
        b.add_action(Action::Click);
        nodes.push((NodeId(i), b));
    }
    TreeUpdate { nodes, tree: Some(TreeInfo::new(ROOT)), tree_id: TreeId::ROOT, focus: ROOT }
}

struct Activation(usize);
impl ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        Some(tree(self.0))
    }
}

struct Handler(usize);
impl ActionHandler for Handler {
    fn do_action(&mut self, r: ActionRequest) {
        let now = CUR.fetch_add(1, SeqCst) + 1;
        PEAK.fetch_max(now, SeqCst);
        CALLS.fetch_add(1, SeqCst);
        let tid = unsafe { GetCurrentThreadId() };
        if tid != UI_TIDS[self.0].load(SeqCst) {
            OFF.fetch_add(1, SeqCst);
        }
        if r.target_node == NodeId(1) && r.action == Action::Click {
            let ms = DELAY.load(SeqCst);
            if ms > 0 {
                println!("slow action on tid={tid} for {ms} ms");
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
        }
        CUR.fetch_sub(1, SeqCst);
    }
}

thread_local! {
    static ADAPTER: RefCell<Option<(usize, Adapter)>> = const { RefCell::new(None) };
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_GETOBJECT => {
            let r = ADAPTER.with(|a| {
                let mut a = a.borrow_mut();
                let (i, ad) = a.as_mut().unwrap();
                ad.handle_wm_getobject(wp, lp, &mut Activation(*i))
            });
            match r {
                Some(r) => r.into(),
                None => unsafe { DefWindowProcW(h, msg, wp, lp) },
            }
        }
        m if m == WM_APP + 1 => {
            std::thread::sleep(std::time::Duration::from_millis(wp.0 as u64));
            LRESULT(0)
        }
        m if m == WM_APP + 2 => {
            let r = PEAK.swap(0, SeqCst) | (OFF.swap(0, SeqCst) << 16) | (CALLS.swap(0, SeqCst) << 40);
            LRESULT(r as isize)
        }
        m if m == WM_APP + 4 => {
            DELAY.store(wp.0 as u64, SeqCst);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

fn main() {
    let n: usize = std::env::args()
        .find_map(|a| a.strip_prefix("threads=").map(|v| v.parse().unwrap()))
        .unwrap_or(1);
    let mut joins = Vec::new();
    for i in 0..n {
        joins.push(std::thread::spawn(move || unsafe {
            UI_TIDS[i].store(GetCurrentThreadId(), SeqCst);
            let inst = GetModuleHandleW(None).unwrap();
            let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst.into(), lpszClassName: w!("PuiaAk"), ..Default::default() };
            RegisterClassW(&wc);
            let title: Vec<u16> = format!("puia-ak-{i}").encode_utf16().chain([0]).collect();
            let h = CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("PuiaAk"), PCWSTR(title.as_ptr()),
                WS_OVERLAPPEDWINDOW, 0, 0, 300, 200, None, None, Some(inst.into()), None,
            )
            .unwrap();
            // The adapter is made before the window can receive WM_GETOBJECT
            // from a client, as AccessKit requires.
            let ad = Adapter::new(h, false, Handler(i));
            ADAPTER.with(|a| *a.borrow_mut() = Some((i, ad)));
            println!("window {i} hwnd={} tid={}", h.0 as isize, GetCurrentThreadId());
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
            }
        }));
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!("ready");
    for j in joins {
        let _ = j.join();
    }
}
