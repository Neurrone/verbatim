# verbatim-inspect

The developer CLI over the control plane. It is not a child of Core and is
in no job object; it attaches through the control pipe, or a TCP address,
like any client, through `verbatim-control`'s `Client`
([verbatim-control](verbatim-control.md)). Its subcommands, options and
output are described once, in
[the inspect tooling guide](../tooling/inspect.md).
