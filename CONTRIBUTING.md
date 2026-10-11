# Contributing to udptap

Thank you for your interest in contributing to `udptap`!

## Development Guidelines

1. **Standalone Crate**: `udptap` is an independent crate with no workspace dependencies.
2. **Quality & Tests**: Ensure all code compiles cleanly without warnings and that existing and new tests pass:
   ```bash
   cargo test
   ```
3. **No External Runtime Daemons**: `udptap` runs entirely in userspace with zero external daemon requirements.
4. **License & Attribution**: Contributions are licensed under the MIT License. Please do not add automated or AI attribution in code comments or commit logs.
