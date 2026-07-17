# Configuration and profiles

NVDA's settings system (`source/config/`) is a layered, validated,
profile-switchable store that most subsystems read live on every use.
Its interesting properties are the layering semantics and the trigger
machinery, both of which any settings design ends up either copying or
consciously rejecting.

## The base store

Settings live in `nvda.ini` in the user configuration directory,
parsed by ConfigObj against a *validation spec*
(`config/configSpec.py`, `confspec`): every key has a declared type,
default, and bounds, so a hand-edited or corrupted value degrades to
its default rather than crashing consumers. The spec carries a schema
version; upgrade steps (`config/profileUpgradeSteps.py`,
`profileUpgrader.py`) migrate old files forward at load. Feature
flags (`config/featureFlag.py`, `featureFlagEnums.py`) are a typed
tri-state pattern (default / explicitly on / explicitly off) used to
ship behavior changes with an escape hatch — the "cancel expired
focus speech" flag in [Speech](speech.md) is one.

Access is dictionary-style (`config.conf["reviewCursor"]["followFocus"]`)
and *live*: consumers read at use time, so most changes apply without
restart. Saving is explicit (`ConfigManager.save`, on exit by default
per the "save configuration on exit" setting); secure mode blocks
saving entirely ([Secure mode](secure-mode.md)). System-wide
parameters (`config/registry.py` reads policy keys like
`forceSecureMode`/`serviceDebug`) override from the registry.

## Profiles: the layering model

A *configuration profile* is a sparse ini file containing only the
keys it overrides. `ConfigManager` keeps a stack: base configuration,
then each active profile in activation order; reads resolve top-down
through `config/aggregatedSection.py` (`AggregatedSection`), which
merges sections key-by-key so a profile overriding one speech setting
inherits everything else. Writes go to the *most recently activated
profile being edited* — the GUI makes the editing target explicit.
Profile files live under `profiles/` in the user config.

Activation is by *trigger* (`config.ProfileTrigger`), with two
built-in kinds plus manual:

- **Manual activation** (`ConfigManager.manualActivateProfile`) — the
  user picks a profile; it stays active until deactivated and outranks
  trigger-activated profiles.
- **App triggers** — a profile bound to an application name activates
  while that app has focus (entered/exited from app-switch handling;
  `appModuleHandler.handleAppSwitch`).
- **Say-all trigger** (`speech/sayAll.py`, `SayAllProfileTrigger`) —
  active during continuous reading, the "different voice for say-all"
  feature.

Trigger bindings persist (`saveProfileTriggers`), can be temporarily
disabled globally (`profileTriggersEnabled`), and nest: base, app
profile, say-all profile can all be active with well-defined
precedence (most recent wins per key).

## Profile switches are events

Because consumers read live, a switch must notify the stateful ones:
`config.post_configProfileSwitch` (an extension point) fires after
the stack changes, and subscribers re-apply themselves — the synth
reloads voice settings, symbol data caches clear
([Symbols and dictionaries](symbols-and-dictionaries.md)), braille
re-tethers. The speech pipeline additionally synchronizes switches
*into the utterance stream* with `ConfigProfileTriggerCommand` so a
say-all profile's voice starts exactly at the say-all's first word,
not mid-utterance ([Speech](speech.md), docstring steps 5 and 9).
This queue-synchronized application is the subtle part of the whole
design — settings that affect an in-flight utterance must not apply
retroactively.

## Related storage

The user configuration directory also holds the add-on store
(`addons/`), speech dictionaries, gesture remaps (`gestures.ini`),
and the developer scratchpad — meaning "copy the user's config"
(portable copies, secure-screen propagation;
[Secure mode](secure-mode.md)) is a directory copy with defined
exclusions, not a single file.
