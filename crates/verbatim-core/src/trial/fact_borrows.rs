//! The async form lends the queue's facts as a borrow of the entry's
//! context, and `ask` borrows that context mutably, so a fact read before
//! an ask cannot be held across its `.await`. The two tests compile the
//! trial's own async form from its source files: the first, which reads a
//! fact after its ask, compiles, so the second fails only for holding the
//! borrow (E0502). Rustdoc checks the error code it names only on a
//! nightly toolchain, so on the pinned stable one the first test is what
//! shows the reason.
//!
//! A value copied out of the facts before an ask, such as
//! `let silent = cx.facts().silent();`, compiles, and is stale after the
//! ask: the borrow rule catches only a held borrow.
//!
//! ```
//! #[path = "harness.rs"]
//! mod harness;
//! mod async_form {
//!     #[path = "driver.rs"]
//!     mod driver;
//!     #[path = "logic.rs"]
//!     mod logic;
//!     #[path = "request.rs"]
//!     mod request;
//!
//!     pub(crate) async fn read_after(mut cx: driver::Cx) -> bool {
//!         let window = crate::harness::Window { top: 1, id: 11 };
//!         let _ = cx.ask(request::Caret { app: 1 }, window, 0).await;
//!         cx.facts().silent()
//!     }
//! }
//! fn main() {}
//! ```
//!
//! ```compile_fail,E0502
//! #[path = "harness.rs"]
//! mod harness;
//! mod async_form {
//!     #[path = "driver.rs"]
//!     mod driver;
//!     #[path = "logic.rs"]
//!     mod logic;
//!     #[path = "request.rs"]
//!     mod request;
//!
//!     pub(crate) async fn held_across(mut cx: driver::Cx) -> bool {
//!         let facts = cx.facts();
//!         let window = crate::harness::Window { top: 1, id: 11 };
//!         let _ = cx.ask(request::Caret { app: 1 }, window, 0).await;
//!         facts.silent()
//!     }
//! }
//! fn main() {}
//! ```
