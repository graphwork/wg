# Fake Pi bounded-evaluation fixtures

`pi` is a credential-free stand-in for Pi's documented non-interactive CLI
contract used by WG: `pi --mode json --print --no-tools -ne --no-session`.
The fixture emits newline-delimited JSON and pins the fields consumed by the
adapter from Pi's `turn_end` event:

```
turn_end.message.role
turn_end.message.content[{type:"text", text}]
turn_end.message.provider
turn_end.message.model
turn_end.message.usage.{input,output,cacheRead,cacheWrite,totalTokens,cost.total}
```

The selected model name chooses valid verdict, malformed output, timeout,
process failure, route drift, duplicate-delivery, or Pi-reported error behavior.
The script also asserts the bounded argv, spotlight prompt, sanitized
environment, and absence of source session/credential variables.
