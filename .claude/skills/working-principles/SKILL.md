---
name: working-principles
description: Core rules for writing or changing code in BlackBox — think before coding, keep it simple, one major thing per file, surgical edits, verify against a concrete goal. Use at the start of any implementation, bug fix, or refactor task.
---

# Working principles

Adapted from skysat-mlops. Bias toward care over speed. For trivial edits (typo, rename) skip the ceremony.

## 1. Think before coding

- State your assumptions. If a request has two plausible readings, name both and ask.
- If a simpler approach than the one requested exists, say so before building the complex one.
- State what a design buys in numbers (lines, commits, fsyncs, memory) before building it. If the gain is small, say so.

## 2. Simplicity first

- Write the least code that solves the stated problem. No abstraction for a single use, no knob nobody asked for.
- If 200 lines could be 50, rewrite it. The developer must be able to say why every line exists.
- The developer is a student and owns this code: code they cannot read is a defect, however correct it is.

## 3. One major thing per file

- A folder per subsystem, a file per major concept: a type and its own methods together, the code that uses it elsewhere.
  Rust 2018 layout: `name.rs` beside `name/`, no `mod.rs`. `name.rs` holds the subsystem's overview comment, its
  `mod` list and its `pub use` list, little else; never a `utils`/`helpers` file, name files by their job.
- A file over ~300 lines usually holds two things: split it.

## 4. Surgical changes

- Touch only what the request requires. Match the surrounding style and comment density.
- Mention unrelated bugs or dead code; do not fix them unprompted.
- Clean up only the mess your own change created.

## 5. Goal-driven execution

- Turn the task into a check before starting: "fix the bug" becomes "a test that fails, then passes".
- Run the check and report the real output. Never "should work".
- When a result looks surprisingly good, suspect the measurement first.
