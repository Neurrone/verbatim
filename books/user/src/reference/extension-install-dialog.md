# What an extension's install dialog says

Extensions are not available yet. This page describes them as they are being built.

Verbatim works out what an extension can do from the extension itself, not from what its author says, and the install dialog lists it. Lines that need your attention come first.

| Line | What it means |
|---|---|
| Runs native code in a restricted process of its own, and receives all the text Verbatim speaks | The extension contains ordinary Windows code, usually a voice whose engine exists only in that form. It still runs restricted, but such code is harder to contain. |
| Receives all the text Verbatim speaks, and can connect to *sites* | Anything Verbatim reads to you, from any application, could be sent to those sites. |
| Reads the controls and text of *applications*, and can connect to *sites* | What you have open in those applications could be sent to those sites. |
| … can connect to the address you enter | As above, to the address you type into the extension's settings, such as your own server. |
| Changes what Verbatim says before it is spoken | The extension rewrites speech, for example translating it. |
| Reads the controls and text of *applications* | The extension reads those applications to change how Verbatim speaks them. |
| Runs its own code when something happens in *applications* | The extension reacts to what happens in those applications. |
| Changes how Verbatim reads *applications* with *N* rules, with no code of its own | The extension only adjusts how controls are spoken, without running any code. |
| Adds keyboard commands in *applications* | One line for each command, with its keys. |
| Provides a speech synthesizer | The extension adds a voice. |
| Drives *display* | The extension supports that braille display, and can use only that device. |
| Needs files you add yourself | The extension needs files you must supply, such as a voice's engine files, which the Extensions dialog asks for. |

A line that says an extension "can connect" describes what it is able to do, not what it does. Every extension can also keep files in a folder of its own, which is not listed, and nothing else on your computer.
