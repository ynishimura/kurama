# Functional Programming Architecture

The principles behind kurama's layers. The rules themselves are enforced by
`tests/architecture/` and summarized in `AGENTS.md` (Layout) and
[module-structure.md](module-structure.md).

## Core Principles

| Principle | Description | Rust Implementation |
|-----------|-------------|---------------------|
| **Pure Functions** | Same input → Same output, no side effects | `fn f(x: T) -> U` |
| **Immutability** | Data is never mutated | `let` (immutable by default) |
| **Effect Separation** | I/O and state changes only at boundaries | Ports & Adapters |
| **Type-Driven Design** | Prevent invalid states at type level | newtype pattern |
| **Function Composition** | Combine small functions into larger operations | `.map()`, `.and_then()` |
| **Algebraic Data Types** | Sum and Product types | `enum` / `struct` |

## Architecture Pattern: Functional Core, Imperative Shell

```
┌─────────────────────────────────────────────────────────────┐
│                      SHELL (Impure)                         │
│   main.rs, CLI handlers, Effect execution                   │
│   ─────────────────────────────────────────────────────     │
│   Side effects are executed only here                       │
├─────────────────────────────────────────────────────────────┤
│                    WORKFLOWS (Pure)                         │
│   State machines: assume_role, mfa_login                    │
│   ─────────────────────────────────────────────────────     │
│   fn step(state, event) -> (state, effects)                 │
├─────────────────────────────────────────────────────────────┤
│                     DOMAIN (Pure)                           │
│   Types, Validation, Business Rules                         │
│   ─────────────────────────────────────────────────────     │
│   Credentials, Profile, SessionDuration                     │
├─────────────────────────────────────────────────────────────┤
│                     PORTS (Traits)                          │
│   Abstraction of side effects (interfaces)                  │
│   ─────────────────────────────────────────────────────     │
│   StsOperations, MfaProvider, SessionCache, TokenStore      │
├─────────────────────────────────────────────────────────────┤
│                   ADAPTERS (Impure)                         │
│   AWS SDK, 1Password CLI, macOS keychain, files             │
│   ─────────────────────────────────────────────────────     │
│   Concrete side effect implementations                      │
└─────────────────────────────────────────────────────────────┘
```

## Directory Structure

The tree is not reproduced here because a copied tree drifts from the code.
The layers are the top-level modules of `src/` (`domain`, `workflows`,
`ports`, `adapters`, and `shell`). Configuration bootstrap and small
application-specific utilities live under `adapters/` or `shell/`. For the
files that implement one feature run
`cargo xtask map <feature>`; the mapping lives in `.agent/features/` and
is validated by `tests/architecture/`, which also enforces the layer rules.
