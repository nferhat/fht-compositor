# Developing/Contributing

The Nix flake provides a `devShell` with everything needed to develop `fht-compositor`. You should
use an editor that supports the Language Server Protocol to get the full power of
[Rust Analyzer](https://rust-analyzer.github.io/)

> ⚠️ **Warning**: Do not overwrite `CARGO_BUILD_RUSTFLAGS` in your shell!
>
> This should only be set with `CARGO_BUILD_RUSTFLAGS="$CARGO_BUILD_RUSTFLAGS -C your-flags"`. The
> provided flags are **needed** to link the compositor properly with the system libraries.

## Testing the Nix package

If you need to test the Nix package build output, you should use the `fht-compositor-debug` package
provided by the flake to have faster build cycles, since it does not enable aggresive optimization
and doesn't strip the final binary.

## Profiling

If you need to do profiling, you can using [Tracy](https://github.com/wolfpld/tracy)

> ⚠️ **Warning**: Enabling profiling will start sending extensive amounts of data, and stores a
> **lot** of marks! The memory footprint of the compositor will grow quite drastically.
>
> You should NOT use a profiled package as your daily driver!

```nix
{
  programs.fht-compositor.package = fht-compositor.override {withProfiling = true;};
}
```
