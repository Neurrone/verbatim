# Reference

The reference describes the extension API as designed for milestone M5. Once the API's WIT files and manifest types exist, the pages on manifests, hooks, events and imports are generated from them.

## Writing a module

- [The manifests](manifests.md): every key of `manifest.toml` and `module.toml`, and what is refused.
- [Matchers](matchers.md): the one grammar for matching windows, events and controls.
- [Rules](rules.md): the window, event and node rules, their actions, where they run and how they combine.
- [Hooks](hooks.md): the functions Verbatim calls in your component, and their verdicts.
- [Events](events.md): the event kinds and what each carries.
- [Commands and gestures](commands.md): declaring commands, gestures with several presses, and what `on-command` receives.
- [Imports](imports.md): every function your component can call.

## Security and permissions

- [Capabilities and the install dialog](capabilities.md): what each group of imports gives an extension and what the user is shown for it.

## Reliability and performance

- [Limits](limits.md): deadlines, the breaker and resource limits.
