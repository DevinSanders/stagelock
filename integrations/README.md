# Broadcast Ecosystem Integrations

External controller and studio surface integrations for **VideoController**.

| Directory | Description |
| :--- | :--- |
| [`companion-module/`](./companion-module/README.md) | First-party **Bitfocus Companion & Bitfocus Buttons** TypeScript module (`@companion-module/base`) supporting Stream Deck keys, **Stream Deck + rotary dials**, real-time WebSocket feedbacks, dynamic variables, and ready-to-drag presets. |

## Built-In Rust Integrations

Core daemon-side integrations (vMix TCP Tally + HTTP XML switching, Bitfocus Buttons Control API client, OSC UDP `:9000` listener, and optional Direct USB Stream Deck coexistence toggle) live in [`crates/vc-integrations/`](../crates/vc-integrations/README.md) and [`crates/vc-daemon/`](../crates/vc-daemon/README.md).
