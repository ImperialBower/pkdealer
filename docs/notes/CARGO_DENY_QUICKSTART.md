# Quick Start: Using cargo-deny Without Warnings

## Current Status: ✅ No Warnings!

Your `deny.toml` is configured to run cleanly without warnings.

```bash
$ cargo deny check
advisories ok, bans ok, licenses ok, sources ok
```

## How to Add Dependencies

### Step-by-Step Example

Let's say you want to add the `serde` crate (MIT OR Apache-2.0):

#### 1. Add the dependency
```bash
cargo add serde
```

#### 2. Check for license issues
```bash
cargo deny check licenses
```

#### 3. If you see an error like this:
```
error[rejected]: failed to satisfy license requirements
  license Foo is not explicitly allowed
```

#### 4. Open `deny.toml` and add the needed license to the `allow` list

#### 5. Verify it works:
```bash
cargo deny check
# Should show: licenses ok
```

## Project Licensing Policy

This project is dual-licensed under **MIT OR Apache-2.0**. Dependency
licenses must be compatible with that pairing — i.e. permissive,
without copyleft obligations.

### Allowed (permissive)
- MIT
- Apache-2.0 (with or without LLVM-exception)
- BSD-2-Clause, BSD-3-Clause, 0BSD, ISC
- CC0-1.0, Unlicense, BSL-1.0
- Zlib, MPL-2.0
- Unicode-3.0, Unicode-DFS-2016

### Disallowed (copyleft)
Avoid adding crates licensed under GPL-2.0, GPL-3.0, LGPL, or AGPL —
they would force this crate's downstream consumers to adopt those
terms, breaking the MIT/Apache dual-license guarantee.

### Watch Out For
- **OpenSSL** — prefer the `rustls` feature instead.
- **BSD-4-Clause** — has an advertising clause; avoid.

## Tips

### 🧹 Duplicate-Version Skips
`deny.toml` currently uses `[bans].skip` for a few older transitive crates required by the current `tonic`/`prost` stack. This keeps `cargo deny check` output clean while `multiple-versions = "warn"` still catches new duplicates.

### ♻️ When To Remove Skips
After upgrading dependencies, run:
```bash
cargo deny check bans
```
If the older versions are no longer in `Cargo.lock`, remove the matching entries from `[bans].skip` in `deny.toml`.

### 🔍 Check Before Adding
Look at the crate's license on crates.io or its repository before adding it.

## Commands

```bash
# Check everything
cargo deny check

# Check only licenses
cargo deny check licenses

# Check only security advisories
cargo deny check advisories

# Using Makefile
make audit
```

## More Information

- **Configuration:** `deny.toml`
- **SPDX license list:** https://spdx.org/licenses/

## TL;DR

1. ✅ Configuration is ready - no warnings
2. 📦 Add dependencies as normal with `cargo add`
3. ⚠️ If license rejected, add it to `allow` in `deny.toml` (must be permissive)
4. ✅ Run `cargo deny check` to verify
5. 🎉 Done!
