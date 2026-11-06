# Limits

The rules behind these limits are explained in [Reliability](../explanation/reliability.md). A limit marked "not set" has its value chosen from measurements of Verbatim's own modules before extensions ship.

## Deadlines

| Limit | Value |
|---|---|
| Time to handle one event, reads included | 10 seconds, shared with Verbatim's own reading |
| Time in your own code per call, reads excluded | Not set |
| Deadline per speech-text stage | Not set |

## The breaker

| Limit | Value |
|---|---|
| Failures to respond that turn an extension off | 3 within 1 minute |
| How long it stays off | Until the session ends, or the user turns it back on |

## Resources

| Limit | Value |
|---|---|
| Memory per extension | Not set |
| Processor time per extension | Not set |
| Data folder size | Not set |
| Speech, browseable messages and log lines | Rates and sizes not set |
| Tones | Duration and frequency capped, values not set |

## Packages

| Limit | Value |
|---|---|
| Files in a `.vext` | 256 |
| Size of one file | 16 MiB; a raised limit for speech-text stages that carry a language model is not set |
| Size unpacked | 64 MiB |
| File names | Plain names only: no `..`, drive, absolute path, link, colon, reserved Windows device name, or trailing dot or space |
