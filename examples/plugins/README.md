# Chatify Plugin Examples

Chatify plugins are executable processes. The server invokes them with:

- `--chatify-plugin-op manifest`
- `--chatify-plugin-op slash --chatify-plugin-command <name>`
- `--chatify-plugin-op message_hook`

For `slash` and `message_hook`, the server writes a JSON payload to stdin and expects a JSON object on stdout.

## hello-plugin.cmd

`hello-plugin.cmd` is a tiny Windows-friendly reference plugin for the v1 plugin API. It wraps `hello-plugin.ps1`, registers `/hello`, and returns a channel message.

Install it from the client:

```text
/plugin install C:\absolute\path\to\Chatify\examples\plugins\hello-plugin.cmd
```

Then run:

```text
/hello reviewer
```

Expected behavior:

- `manifest` returns API version `1`, plugin name `hello-plugin`, and command metadata.
- `slash` reads `channel`, `user`, `command`, and `args` from stdin.
- The response contains a `messages` array with a channel-targeted message.

## Response Shape

Minimal manifest response:

```json
{
  "api_version": "1",
  "name": "hello-plugin",
  "message_hook": false,
  "commands": [
    {
      "name": "hello",
      "description": "Say hello from an external plugin"
    }
  ]
}
```

Minimal slash response:

```json
{
  "api_version": "1",
  "messages": [
    {
      "target": "channel",
      "text": "Hello from the plugin"
    }
  ]
}
```
