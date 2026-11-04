# Verbatim speech transcript: an NVDA global plugin.
# This file is covered by the GNU General Public License, version 3 or later.
# See the LICENSE file at the root of the Verbatim repository.

"""Serves a transcript of NVDA's speech to Verbatim's development tooling.

A research instrument, not a test oracle: `docs/nvda-transcript.md` in the
Verbatim repository describes how it is used. The plugin listens on
127.0.0.1 only, on port 44100 plus the Windows session id of the NVDA that
loaded it (so each session's NVDA has its own port), accepts one client at a
time, and records only while that
client is connected; the buffer is cleared when the client disconnects.

The wire format is the one Verbatim's agent speaks: one compact JSON object
per line. Every request is an envelope `{"id": n, "request": ...}` and every
answer is `{"Reply": {"to": n, "payload": ...}}` or
`{"Error": {"to": n, "message": ...}}`. Requests:

- `{"Hello": {"protocol_version": 1}}`, which must come first, answered with
  `{"Hello": {"protocol_version": 1, "nvda_version": "...", "session_id": n}}`;
  recording
  starts here.
- `{"Read": {"after": seq}}`, answered with `{"Entries": {"entries": [...]}}`
  holding every entry whose sequence number is greater than `seq`.

An entry is `{"seq": n, "ms": t, "event": ...}`, where `ms` counts
milliseconds since the `Hello`, and the event is
`{"Speech": {"text": "...", "priority": "Normal"}}` for a queued speech
sequence or the string `"Cancel"` when speech is cancelled. Speech text is
flattened the way NVDA's own system-test spy does it: the string items of
the sequence joined and stripped, so commands such as pitch changes and
index marks are dropped.
"""

import collections
import ctypes
import json
import socket
import threading
import time

import buildVersion
import globalPluginHandler
from logHandler import log
from speech.extensions import pre_speechQueued, speechCanceled

PROTOCOL_VERSION = 1
#: The port is this plus the session id; see `_sessionId`.
PORT_BASE = 44100
#: Entries kept while a client is connected; the oldest are dropped first.
MAX_ENTRIES = 10000

_PRIORITY_NAMES = {0: "Normal", 1: "Next", 2: "Now"}


def _sessionId():
	"""The Windows session this NVDA runs in.

	One user can have NVDA running in two sessions at once (a Remote Desktop
	session and the console, say), and both load this plugin; a port per
	session keeps each one reachable.
	"""
	sessionId = ctypes.c_ulong()
	kernel32 = ctypes.windll.kernel32
	if not kernel32.ProcessIdToSessionId(kernel32.GetCurrentProcessId(), ctypes.byref(sessionId)):
		raise ctypes.WinError()
	return sessionId.value


class _Recorder:
	"""The transcript buffer, written on NVDA's main thread and read on the server thread."""

	def __init__(self):
		self._lock = threading.Lock()
		self._entries = collections.deque(maxlen=MAX_ENTRIES)
		self._nextSeq = 1
		self._start = 0.0
		self.recording = False

	def start(self):
		with self._lock:
			self._entries.clear()
			self._start = time.perf_counter()
			self.recording = True

	def stop(self):
		with self._lock:
			self.recording = False
			self._entries.clear()

	def _append(self, event):
		with self._lock:
			if not self.recording:
				return
			ms = int((time.perf_counter() - self._start) * 1000)
			self._entries.append({"seq": self._nextSeq, "ms": ms, "event": event})
			self._nextSeq += 1

	def onSpeechQueued(self, speechSequence, priority, **kwargs):
		if not self.recording:
			return
		text = "".join(item for item in speechSequence if isinstance(item, str)).strip()
		self._append({"Speech": {"text": text, "priority": _PRIORITY_NAMES.get(int(priority), str(priority))}})

	def onSpeechCanceled(self, **kwargs):
		if not self.recording:
			return
		self._append("Cancel")

	def after(self, seq):
		with self._lock:
			return [entry for entry in self._entries if entry["seq"] > seq]


class _Server(threading.Thread):
	def __init__(self, recorder, sessionId):
		super().__init__(name="verbatimTranscript", daemon=True)
		self._recorder = recorder
		self._sessionId = sessionId
		self._listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
		# NVDA sets a ten-second default timeout on every socket at startup,
		# which would end an idle accept, or an idle client, with an error.
		self._listener.settimeout(None)
		self._listener.bind(("127.0.0.1", PORT_BASE + sessionId))
		self._listener.listen(1)
		self._stopping = False

	def stop(self):
		self._stopping = True
		try:
			self._listener.close()
		except OSError:
			pass

	def run(self):
		while not self._stopping:
			try:
				connection, _address = self._listener.accept()
			except OSError:
				if self._stopping:
					return
				log.error("verbatimTranscript: accept failed", exc_info=True)
				time.sleep(1)
				continue
			connection.settimeout(None)
			try:
				self._serve(connection)
			except Exception:
				log.error("verbatimTranscript: client connection failed", exc_info=True)
			finally:
				self._recorder.stop()
				connection.close()

	def _serve(self, connection):
		reader = connection.makefile("r", encoding="utf-8", newline="\n")
		greeted = False
		for line in reader:
			if not line.strip():
				continue
			envelope = json.loads(line)
			requestId = envelope["id"]
			request = envelope["request"]
			kind = next(iter(request)) if isinstance(request, dict) else request
			if kind == "Hello":
				if request["Hello"]["protocol_version"] < PROTOCOL_VERSION:
					self._send(connection, {"Error": {
						"to": requestId,
						"message": f"protocol version {PROTOCOL_VERSION} required",
					}})
					return
				greeted = True
				self._recorder.start()
				payload = {"Hello": {
					"protocol_version": PROTOCOL_VERSION,
					"nvda_version": buildVersion.version,
					"session_id": self._sessionId,
				}}
			elif not greeted:
				self._send(connection, {"Error": {"to": requestId, "message": "Hello must come first"}})
				return
			elif kind == "Read":
				payload = {"Entries": {"entries": self._recorder.after(request["Read"]["after"])}}
			else:
				self._send(connection, {"Error": {"to": requestId, "message": f"unknown request {kind}"}})
				continue
			self._send(connection, {"Reply": {"to": requestId, "payload": payload}})

	@staticmethod
	def _send(connection, frame):
		line = json.dumps(frame, ensure_ascii=False, separators=(",", ":")) + "\n"
		connection.sendall(line.encode("utf-8"))


class GlobalPlugin(globalPluginHandler.GlobalPlugin):
	def __init__(self):
		super().__init__()
		self._recorder = _Recorder()
		pre_speechQueued.register(self._recorder.onSpeechQueued)
		speechCanceled.register(self._recorder.onSpeechCanceled)
		self._server = None
		sessionId = _sessionId()
		try:
			self._server = _Server(self._recorder, sessionId)
		except OSError:
			log.warning(f"verbatimTranscript: port {PORT_BASE + sessionId} is in use; not serving", exc_info=True)
			return
		self._server.start()

	def terminate(self):
		pre_speechQueued.unregister(self._recorder.onSpeechQueued)
		speechCanceled.unregister(self._recorder.onSpeechCanceled)
		if self._server is not None:
			self._server.stop()
		super().terminate()
