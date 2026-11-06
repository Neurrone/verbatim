# Installing and managing extensions

Extensions are not available yet. This page describes them as they are being built.

Before you install an extension, read [Extensions and your privacy](../explanation/extensions-and-privacy.md), which explains what an extension can and cannot do.

## Install an extension

1. Make sure Verbatim is running.
2. Open the extension's `.vext` file, for example by pressing Enter on it in File Explorer. If Verbatim is not running, a message says so, and nothing is installed.
3. Verbatim shows the install dialog: the extension's name, version and author, its description, and what it can do. Read the lines that begin "Receives" or "Runs native code" carefully; [What an extension's install dialog says](../reference/extension-install-dialog.md) explains each line.
4. Choose Install, or Cancel to install nothing.

## Update an extension

Open the newer version's `.vext` file. If the new version can send your content out in a new way, connect to a new site or use a new device, Verbatim lists only what is new and asks you first. Choose Update, or Keep the current version.

## See what your extensions can do

Open the Verbatim menu and choose Extensions. The list shows each extension's name, version and status. Its details show its description, what it can do, and any problems it has had.

## Turn an extension off or remove it

In the Extensions dialog, select the extension, then:

- choose Disable to turn it off without removing it, and Enable to turn it back on;
- choose Uninstall and confirm to remove it.

## When an extension stops working

If an extension stops responding three times within a minute, Verbatim turns it off until you restart Verbatim and tells you which one. To use it again, turn it back on in the Extensions dialog. If it keeps happening, report it to the extension's author.

## Start Verbatim with every extension off

If an extension makes Verbatim hard to use, start Verbatim with every extension off: hold the startup key while Verbatim starts, or start it with the command-line switch. Verbatim says that extensions are off. Then remove or disable the extension in the Extensions dialog, and restart Verbatim. The key and the switch are named here once they are chosen.

## Allow an extension on sign-in and administrator screens

Extensions do not run on the sign-in screen, the lock screen or the screens that ask for administrator permission unless you allow them there. Allowing one needs administrator approval and applies to every user of the computer. The dialog that allows it lists what the extension can do there; an extension that can connect to the internet could send what Verbatim reads on those screens.
