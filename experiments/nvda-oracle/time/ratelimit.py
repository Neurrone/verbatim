"""A model of NVDA's native UIA event limiter (nvdaHelper/local/UIAEventLimiter/
rateLimitedEventHandler.cpp, eventRecord.h), for runs on simulated time.

The native handler has no clock: each event is appended to a list under a lock. When an event's
coalescing key (the sender's runtime id plus the event id, or plus the property id for a property
change, or plus kind, processing and activity id for a notification) matches a queued one, the
code means to drop the older one (its log says "erasing old item"), but it erases through a
structured-binding reference into the map entry it has just overwritten with the new record's
position, so the record erased is always the new one: the first event of a key in a batch is kept,
at its place, and later ones are dropped. The installed nvdaHelperLocal.dll does this
(run_ratelimit.py). The first event into an empty list wakes a flusher thread, which swaps the list
out and emits it in order to NVDA's handler; events that arrive while it emits form the next batch.
A batch is therefore whatever arrived between two flusher wake-ups: in simulation, what the client
threads queued before the scheduler ran the flusher."""


class Limiter:
	def __init__(self, key=lambda rec: rec):
		self.records = []  # in emit order
		self.key = key
		self.needsFlush = False

	def queue(self, rec):
		wasEmpty = not self.records
		if all(self.key(r) != self.key(rec) for r in self.records):
			self.records.append(rec)
		if wasEmpty:
			self.needsFlush = True

	def flushBatch(self):
		batch, self.records, self.needsFlush = self.records, [], False
		return batch
