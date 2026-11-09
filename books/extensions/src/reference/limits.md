# Limits

The rules behind these limits are explained in [Reliability](../explanation/reliability.md). A limit marked "measured" has its value set from measurements of Verbatim's own modules before extensions ship.

## Deadlines

| Limit | Value |
|---|---|
| Time to handle one event, reads included | 11 seconds, shared with Verbatim's own reading |
| Time in your own code per call, reads excluded | Measured; deliberately loose until then |
| Deadline per speech-text stage | Not set |

## The breaker

| Limit | Value |
|---|---|
| Failures to respond that turn an extension off | 3 within 1 minute |
| How long it stays off | Until the session ends, or the user turns it back on |

## Resources

| Limit | Value |
|---|---|
| Memory for each module instance | 64 MiB, measured |
| Memory for the extension's process | 256 MiB, measured |
| Data folder size | Not set |

There are no limits on how much or how often you speak, show messages, play tones or log, or on list dialogs, as in NVDA. A long message costs only the time it takes to speak.

## Packages

| Limit | Value |
|---|---|
| File names | Plain names only: no `..`, drive, absolute path, link, colon, reserved Windows device name, or trailing dot or space |

There are no limits on the number or size of files in a `.vext`.
