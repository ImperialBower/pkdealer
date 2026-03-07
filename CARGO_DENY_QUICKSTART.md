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
  license MIT is not explicitly allowed
  license Apache-2.0 is not explicitly allowed
```

#### 4. Open `deny.toml` and uncomment the needed licenses:

**Before:**
```toml
allow = [
    "GPL-3.0-or-later",
]
```

**After:**
```toml
allow = [
    "GPL-3.0-or-later",
    "MIT",
    "Apache-2.0",
]
```

#### 5. Verify it works:
```bash
cargo deny check
# Should show: licenses ok
```

## Common Rust Dependency Licenses

Most Rust crates use one of these patterns:

### Pattern 1: Dual Licensed (Most Common)
```toml
# Crate uses: "MIT OR Apache-2.0"
# Uncomment in deny.toml:
allow = [
    "GPL-3.0-or-later",
    "MIT",
    "Apache-2.0",
]
```

### Pattern 2: MIT Only
```toml
# Crate uses: "MIT"
# Uncomment in deny.toml:
allow = [
    "GPL-3.0-or-later",
    "MIT",
]
```

### Pattern 3: Apache-2.0 Only
```toml
# Crate uses: "Apache-2.0"
# Uncomment in deny.toml:
allow = [
    "GPL-3.0-or-later",
    "Apache-2.0",
]
```

## Available GPL-Compatible Licenses

All these are ready to uncomment in your `deny.toml`:

**Most Common:**
- MIT
- Apache-2.0
- Apache-2.0 WITH LLVM-exception

**BSD Family:**
- BSD-2-Clause
- BSD-3-Clause
- 0BSD
- ISC

**GPL Family:**
- GPL-3.0-only
- LGPL-3.0, LGPL-2.1
- LGPL-2.1-or-later, LGPL-3.0-or-later
- AGPL-3.0

**Public Domain:**
- CC0-1.0
- Unlicense

**Other:**
- BSL-1.0 (Boost)
- Zlib
- MPL-2.0
- Unicode-DFS-2016

## Tips

### ✅ Best Practice
Start with MIT and Apache-2.0 since ~90% of Rust crates use these:
```toml
allow = [
    "GPL-3.0-or-later",
    "MIT",
    "Apache-2.0",
]
```

### ⚠️ Watch Out For
- **GPL-2.0-only** - NOT compatible (needs "or-later")
- **OpenSSL** - Use `rustls` feature instead
- **BSD-4-Clause** - NOT compatible

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

- **Full guide:** `GPL_LICENSE_COMPATIBILITY.md`
- **Configuration:** `deny.toml`
- **GNU GPL info:** https://www.gnu.org/licenses/license-list.html

## TL;DR

1. ✅ Configuration is ready - no warnings
2. 📦 Add dependencies as normal with `cargo add`
3. ⚠️ If license rejected, uncomment it in `deny.toml`
4. ✅ Run `cargo deny check` to verify
5. 🎉 Done!

