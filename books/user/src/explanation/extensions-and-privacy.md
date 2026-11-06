# Extensions and your privacy

Extensions are not available yet. This page describes them as they are being built.

Extensions add to what Verbatim can do: support for an application, a voice, a braille display, or translation of what Verbatim speaks. An extension is written by someone other than the Verbatim project, and Verbatim does not review extensions, so installing one means trusting its author. This page explains what that trust covers and what it does not.

## What an extension can never do

Whatever it asks for, an extension cannot:

- read, change or delete your documents or any other file of yours, or another extension's; it has one folder of its own to keep files in;
- install programs, start programs, or run when Verbatim is not running;
- change Verbatim itself, or its own code after you installed it;
- act with administrator rights, or run on sign-in and administrator screens unless you allow it there;
- use a braille display or other device that it does not drive.

Verbatim enforces these limits itself, with the help of Windows, so they hold even if an extension is badly written or its author's account is stolen.

## What an extension can do

Everything else is listed when you install it, in plain sentences; [What an extension's install dialog says](../reference/extension-install-dialog.md) explains each one. Read carefully the lines that say an extension receives something and can connect somewhere:

- "Receives all the text Verbatim speaks, and can connect to api.example.com" means that anything Verbatim reads to you, from any application, could be sent to that site. That includes your email, your documents and what you type, but not what you type into password fields, which Verbatim speaks only as "star", unless an extension changes how that application's password fields are read; Verbatim's focus report then names the extension.
- "Reads the controls and text of Notepad, and can connect to api.example.com" means the same for what you have open in Notepad.

Verbatim can tell you what an extension is able to do, not what it does. A translation extension has to send text to its service to translate it; whether you are comfortable with that depends on who runs the service. Treat it like pasting the text into that website yourself.

An extension that reads an application but cannot connect anywhere can change how Verbatim speaks that application, but has nowhere to send what it reads.

An extension marked "Runs native code" contains ordinary Windows code, such as a voice whose engine exists only in that form. Verbatim still runs it restricted, in a process of its own, but such code is harder to contain, so install it only from authors you trust.

## When an extension asks for more

If an update would let an extension send your content out in a new way, connect to a new site, use a new device, or receive more of your content when it can already send content out, Verbatim asks you before updating, and you can keep the version you have.

## Sign-in and administrator screens

Extensions do not run on the sign-in screen, the lock screen or the screens that ask for administrator permission, unless you allow them there. If you allow an extension that can connect to the internet, what Verbatim reads on those screens could be sent too.

## When something goes wrong

- If an extension stops responding three times within a minute, Verbatim turns it off until you restart Verbatim, and tells you which extension it turned off.
- If an extension makes Verbatim hard to use, start Verbatim with every extension off, then remove the extension.

[Installing and managing extensions](../how-to/extensions.md) gives the steps for both.
