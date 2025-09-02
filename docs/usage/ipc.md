# IPC

`fht-compositor` features a inter-process communication system allowing you fetch data from the
compositor and modify the live compositor state.

You can communicate with this IPC with one of:

1. `fht-compositor ipc` subcommand in the CLI, `fht-compositor ipc --help` for more information

> [!NOTE]
> Version mismatches between the compositor and the CLI may cause errors parsing, so when updating
> make sure to restart your session to make sure both the CLI and compositor are up-to-date.
>
> `fht-compositor ipc version` prints both versions, you can use it to check for mismatches.
>
> ```sh
> $ fht-compositor ipc version
> # Here, both the versions are matching, no need to restart
> Compositor: 25.03.1 (a17380aa)
> CLI: 25.03.1 (a17380aa)
> ```

2. Programmatic access to the IPC, through its Unix socket, useful for scripting or integrating
   with other parts of your system:
   - If you are using Rust, use the `fht-compositor-ipc` library directly, as it has all the
     IPC types pre-defined with `(De)Serialize`.
   - You can do shell scripting using `socat`, though you'll have to figure out what to write
     inside the socket yourself.

> [!TIP]
> To see how the requests/responses are formatted, you can setup and temporary socket and link it
> with the real one using `socat`.
>
> ```sh
> socat -v UNIX-LISTEN:~/temp.sock,fork UNIX-CONNECT:${FHTC_SOCKET_PATH}
> # Then, in another terminal, you can connect to ~/temp.sock
> FHTC_SOCKET_PATH=~/temp.sock fht-compositor ipc version
> ```
>
> You can also use `fht-compositor ipc --json` to see JSON-formatted responses if you only care about
> that.

## Streaming State updates

The IPC has support for sending live events and updates to all subscribed clients.

You can subscribe to the IPC by using the `--subscribe` flag.

> [!NOTE]
>
> As of now, the IPC only supports subscribing to `workspace`, `space`, `layershells`, `window` and `windows`.

::: tabs
== Example with eww

```scheme
; Here, we store the space data into a global variable, updated each second.
; Since eww supports native JSON decoding, this is really handy
(deflisten space-data ; Initital value is empty, to make eww startup fast
                      :initial "{\"monitors\": {}, \"primary-idx\": -1, \"active-idx\": -1}"
                      `fht-compositor ipc --subscribe --json space`)

; Now, somewhere else, you can immediatly use the data to your liking
(label :text "Output: ${space-data.monitors[0].name}")
```

== Example with Quickshell
Taken from [Isabel's Dotfiles](https://github.com/isabelroses/dotfiles)

```qml
Process {
  id: getSelectedWorkspace
  running: true
  command: ["fht-compositor", "ipc", "--json", "focused-workspace"]
  stdout: StdioCollector {
    onStreamFinished: {
      var jsonData = JSON.parse(text);
      // You can now set the actual value somewhere...
      someVariable = jsonData.id
    }
  }
}
```
