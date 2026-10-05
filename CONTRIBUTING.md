# Contributing

Reports from other adapters are the most useful contribution: open an
[adapter report](https://github.com/dacevedo12/jms578-usb2-fix/issues/new?template=adapter-report.yml) with the output
of `sudo jms578-usb2-fix status`, whether or not the fix worked.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --simulate          # the full guided flow against the simulated adapter
uvx ruff check research && uvx ruff format --check research && uvx pytest
```

Ground rules:

- Never commit flash dumps (`*.bin` is ignored): they contain JMicron's firmware and device serial numbers.
- Anything that writes to an adapter must be covered by a test against the simulator in `src/sim.rs`, including the
  failure cases.
- If you support a new firmware or flash chip, say what hardware you verified it on.
