# Plugin API v1

Chatify plugins are executable workers. The server starts a plugin process with
operation flags, sends JSON on stdin for payload-bearing operations, and expects
one JSON object on stdout.

## Worker Operations

The server invokes workers with:

```text
<plugin> --chatify-plugin-op manifest
<plugin> --chatify-plugin-op slash --chatify-plugin-command <name>
<plugin> --chatify-plugin-op message_hook
```

Built-in plugins use the same contract through the server executable.

## Manifest Response

```json
{
  "api_version": "1",
  "name": "hello-world",
  "commands": [
    { "name": "hello", "description": "Send a greeting" }
  ],
  "message_hook": false
}
```

Command names register as slash commands. A command named `hello` is invoked by
users as `/hello`.

## Slash Response

Slash operations receive a JSON payload with the user, channel, command, and
arguments. They return messages to emit:

```json
{
  "api_version": "1",
  "messages": [
    {
      "target": "channel",
      "content": "hello from plugin"
    }
  ]
}
```

## Message Hook Response

Message hooks can observe, replace, or block channel messages:

```json
{
  "api_version": "1",
  "replacement": "rewritten text",
  "blocked": false,
  "messages": []
}
```

Return `blocked: true` to stop the original message from being sent.

## Runtime Limits

- API version must be `1`.
- Plugin specs can be built-ins or executable paths.
- Worker stdout and payload sizes are bounded by the server.
- Slow workers are terminated by timeout.
- Invalid JSON, unsupported API versions, and non-zero process exits are treated
  as plugin failures and do not stop core chat availability.

## Operator Flow

```text
/plugin list
/plugin install poll
/plugin install /absolute/path/to/plugin-worker
/plugin disable poll
```

Plugin install and disable require an admin role with manage permission.

