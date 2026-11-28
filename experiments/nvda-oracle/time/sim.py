"""Simulated time and a cooperative, seedable thread scheduler for headless NVDA.

Installed before any NVDA module is imported (boot.py does it first), so that every
`import time`, `from time import perf_counter`, `threading.Event()`, `queue.Queue()` and `wx`
NVDA reaches is the simulated one.

Threads stay real OS threads (COM apartments and comtypes need that), but only one managed thread
runs at a time: it holds the baton until it reaches a blocking point (sleep, Event.wait, Queue.get,
a waitable-object wait, the wx main loop going idle), when the scheduler picks the next runnable
thread. With nothing runnable, simulated time jumps to the earliest deadline. The choice among
several runnable threads is the only freedom, and it is fixed by `policy` ("fifo": the longest
runnable first; "seeded": a seeded random choice). Clock reads advance time by `tick` so that a
spin on the clock terminates; the result is the same on every run.

A thread not started under the scheduler (a native thread calling into Python) is unmanaged: it
uses real primitives, and the scheduler polls for its effects in real time when otherwise idle."""
import time as _time
import threading as _threading
import queue as _queue
import heapq
import random
import types
from collections import deque

REAL_SLEEP = _time.sleep
REAL_PERF = _time.perf_counter
_RealEvent = _threading.Event
_RealThreadStart = _threading.Thread.start
_RealQueue = _queue.Queue

EPOCH = 1_795_000_000.0  # time.time() at simulated zero: fixed, so logs and caches repeat


class Deadlock(RuntimeError):
	pass


class TState:
	def __init__(self, name, order):
		self.name = name
		self.order = order
		self.sem = _threading.Semaphore(0)
		self.state = "ready"  # ready | running | blocked | done
		self.pred = None
		self.deadline = None
		self.readySince = 0
		self.realWait = 0


class Sim:
	def __init__(self):
		self.now = 0.0
		self.tick = 1e-6
		self.policy = "fifo"
		self.rng = random.Random(0)
		self.byIdent = {}
		self.all = []
		self.current = None
		self.seq = 0
		self.trace = []  # (time, thread name) at each switch, for the ordering study
		self.contended = 0  # picks with more than one runnable thread
		self.choices = 0  # of those, picks the seed decided
		self.errors = []  # uncaught exceptions in managed threads
		self.wakeHooks = []  # callables returning a deadline or None: other sim objects' due times

	# -- clock --
	def read(self):
		self.now += self.tick
		return self.now

	# -- threads --
	def me(self):
		return self.byIdent.get(_threading.get_ident())

	def adoptMain(self, name="main"):
		t = TState(name, len(self.all))
		t.state = "running"
		self.all.append(t)
		self.byIdent[_threading.get_ident()] = t
		self.current = t

	def seed(self, seed, policy="seeded"):
		self.rng = random.Random(seed)
		self.policy = policy

	def _runnable(self):
		out = []
		for t in self.all:
			if t.state == "ready":
				out.append(t)
			elif t.state == "blocked":
				if (t.pred is not None and t.pred()) or (t.deadline is not None and t.deadline <= self.now):
					t.state = "ready"
					self.seq += 1
					t.readySince = self.seq
					out.append(t)
		return out

	def _pick(self):
		realWaited = 0.0
		while True:
			ready = self._runnable()
			if ready:
				if len(ready) > 1:
					self.contended += 1
				if self.policy == "fifo" or len(ready) == 1:
					return min(ready, key=lambda t: (t.readySince, t.order))
				ready.sort(key=lambda t: t.order)
				self.choices += 1
				return self.rng.choice(ready)
			deadlines = [t.deadline for t in self.all if t.state == "blocked" and t.deadline is not None]
			deadlines += [d for d in (h() for h in self.wakeHooks) if d is not None]
			deadlines = [d for d in deadlines if d > self.now]
			if deadlines:
				self.now = min(deadlines)
				continue
			# Nothing will happen in simulated time: only an unmanaged thread can wake anyone.
			if realWaited > 5.0:
				raise Deadlock("no runnable thread, no deadline: " + ", ".join(
					f"{t.name}:{t.state}" for t in self.all))
			REAL_SLEEP(0.001)
			realWaited += 0.001

	def switch(self):
		"""Give up the baton (the caller has set its own state) and return when it is given back."""
		me = self.me()
		nxt = self._pick()
		if nxt is not me:
			self.trace.append((round(self.now, 6), nxt.name))
		nxt.state = "running"
		self.current = nxt
		if nxt is not me:
			nxt.sem.release()
			if me.state != "done":
				me.sem.acquire()

	def block(self, pred=None, deadline=None):
		me = self.me()
		me.pred, me.deadline = pred, deadline
		me.state = "blocked"
		self.switch()
		me.pred = me.deadline = None

	def yield_(self):
		me = self.me()
		me.state = "ready"
		self.seq += 1
		me.readySince = self.seq
		self.switch()

	def managed(self):
		t = self.me()
		return t is not None and not t.realWait


SIM = Sim()


# -- time --
def _sleep(secs):
	if not SIM.managed():
		return REAL_SLEEP(secs)
	if secs <= 0:
		return SIM.yield_()
	SIM.block(None, SIM.now + secs)


def install_time():
	_time.time = lambda: EPOCH + SIM.read()
	_time.time_ns = lambda: int((EPOCH + SIM.read()) * 1e9)
	for name in ("perf_counter", "monotonic", "process_time", "thread_time"):
		setattr(_time, name, SIM.read)
		setattr(_time, name + "_ns", lambda: int(SIM.read() * 1e9))
	_time.sleep = _sleep


# -- threading --
class SimEvent(_RealEvent):
	def wait(self, timeout=None):
		if not SIM.managed() or self.is_set():
			return super().wait(timeout)
		SIM.block(self.is_set, None if timeout is None else SIM.now + timeout)
		return self.is_set()


def _start(self):
	t = TState(self.name, len(SIM.all))
	SIM.seq += 1
	t.readySince = SIM.seq
	run = self.run

	def managedRun():
		SIM.byIdent[_threading.get_ident()] = t
		t.sem.acquire()
		try:
			run()
		except BaseException:
			import traceback
			SIM.errors.append(f"thread {t.name}:\n" + traceback.format_exc())
			raise
		finally:
			t.state = "done"
			SIM.switch()

	self.run = managedRun
	me = SIM.me()
	if me is None:
		return _RealThreadStart(self)
	SIM.all.append(t)
	# Thread.start waits for the bootstrap, which runs without the baton: a real wait.
	me.realWait += 1
	try:
		_RealThreadStart(self)
	finally:
		me.realWait -= 1


class SimQueue(_RealQueue):
	def get(self, block=True, timeout=None):
		if not SIM.managed() or not block:
			return super().get(block, timeout)
		deadline = None if timeout is None else SIM.now + timeout
		while self.empty():
			if deadline is not None and SIM.now >= deadline:
				raise _queue.Empty
			SIM.block(lambda: not self.empty(), deadline)
		return super().get_nowait()


def install_threading():
	_threading.Event = SimEvent
	_threading.Thread.start = _start
	_queue.Queue = SimQueue


# -- wx: CallAfter, CallLater, Timer, Yield, and the main loop --
class _WxTimer:
	def __init__(self, owner=None, id=-1):
		self._due = None
		self._interval = 0
		self._oneShot = True
		self._key = None

	def Start(self, milliseconds=-1, oneShot=False):
		if milliseconds >= 0:
			self._interval = milliseconds
		self._oneShot = bool(oneShot)
		WX.schedule(self, SIM.now + self._interval / 1000)
		return True

	StartOnce = lambda self, ms=-1: self.Start(ms, True)  # noqa: E731

	def Stop(self):
		WX.unschedule(self)

	def IsRunning(self):
		return self._key is not None

	def Notify(self):
		pass

	def _fire(self):
		if not self._oneShot:
			WX.schedule(self, SIM.now + self._interval / 1000)
		self.Notify()


class _CallLater(_WxTimer):
	def __init__(self, millis, callableObj, *args, **kwargs):
		super().__init__()
		self._call = (callableObj, args, kwargs)
		self.result = None
		self.Start(millis, True)

	def Notify(self):
		f, a, k = self._call
		self.result = f(*a, **k)

	def Restart(self, millis=None, *args, **kwargs):
		if args or kwargs:
			self._call = (self._call[0], args, kwargs)
		self.Start(self._interval if millis is None else millis, True)

	SetArgs = lambda self, *a, **k: setattr(self, "_call", (self._call[0], a, k))  # noqa: E731
	HasRun = lambda self: self.result is not None  # noqa: E731


class WxLoop:
	"""The main thread's wx event loop: a FIFO of CallAfter calls and a heap of timers."""

	def __init__(self):
		self.calls = deque()
		self.timers = []  # (due, seq, timer)
		self.seq = 0
		self.yielding = False
		self.lock = _threading.Lock()
		SIM.wakeHooks.append(self.nextDue)

	def schedule(self, timer, due):
		with self.lock:
			self.seq += 1
			timer._key = (due, self.seq)
			heapq.heappush(self.timers, (due, self.seq, timer))

	def unschedule(self, timer):
		timer._key = None

	def nextDue(self):
		with self.lock:
			while self.timers and self.timers[0][2]._key != self.timers[0][:2]:
				heapq.heappop(self.timers)
			return self.timers[0][0] if self.timers else None

	def callAfter(self, f, *a, **k):
		with self.lock:
			self.calls.append((f, a, k))

	def runOne(self):
		"""Run one due item: a CallAfter call first, else the earliest due timer."""
		with self.lock:
			item = self.calls.popleft() if self.calls else None
		if item:
			f, a, k = item
			f(*a, **k)
			return True
		due = self.nextDue()
		if due is not None and due <= SIM.now:
			with self.lock:
				_, _, timer = heapq.heappop(self.timers)
				timer._key = None
			timer._fire()
			return True
		return False

	def runUntil(self, t):
		"""Be the main loop until simulated time t, with every due item run."""
		while True:
			if self.runOne():
				continue
			if SIM.now >= t:
				return
			due = self.nextDue()
			SIM.block(lambda: bool(self.calls), min(t, due) if due is not None else t)

	def wxYield(self, *a):
		if self.yielding:
			return False
		self.yielding = True
		try:
			while self.runOne():
				pass
		finally:
			self.yielding = False
		return True


WX = None


def install_wx(StubModule):
	"""A wx module: the event-loop members are simulated, the rest fall to the boot stubs."""
	global WX
	import sys
	WX = WxLoop()
	wx = StubModule("wx")
	wx.__path__ = []
	wx.Timer = _WxTimer
	wx.CallLater = _CallLater
	wx.CallAfter = WX.callAfter
	wx.Yield = WX.wxYield
	wx.SafeYield = WX.wxYield
	wx.GetApp = lambda: types.SimpleNamespace(name="simulated wx.App")
	wx.IsMainThread = lambda: SIM.me() is SIM.all[0]
	sys.modules["wx"] = wx
	return wx


def install(StubModule):
	install_time()
	install_threading()
	SIM.adoptMain("main")
	return install_wx(StubModule)
