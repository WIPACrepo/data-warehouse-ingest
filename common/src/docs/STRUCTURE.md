# Documentation Structure

This directory contains comprehensive documentation for the `common` crate, organized by topic for easy navigation.

## File Organization

### Core Documentation

| File | Size | Purpose |
|------|------|---------|
| [`README.md`](README.md) | ~7.4 KB | Main index with overview, quick start, and architecture context |
| [`overview.md`](overview.md) | ~9 KB | How `common` fits into the Barbican/Citadel architecture |
| [`models.md`](models.md) | ~8 KB | Shared data model: inboxes, statuses, progress, errors |
| [`infrastructure.md`](infrastructure.md) | ~10 KB | Shared utilities: config, DB, error, metrics, middleware |
| [`auth.md`](auth.md) | ~14 KB | JWT/Keycloak auth: prefixed `AuthConfig`, `build_auth_state`, service-to-service tokens, decision D10 |
| [`testing.md`](testing.md) | ~8 KB | Test utilities and conventions |

### Module Files

| File | Purpose |
|------|---------|
| [`mod.rs`](mod.rs) | Rust module documentation |
| [`STRUCTURE.md`](STRUCTURE.md) | This file - documentation organization |

## Documentation Hierarchy

```
common/src/docs/
│
├── README.md                    # Main entry point
│   ├── Quick Start
│   ├── Architecture Overview
│   ├── Module Structure
│   ├── How Services Use common
│   └── Links to all other docs
│
├── overview.md                  # Position within the system
│   ├── Role in the Architecture
│   ├── Ownership Boundary
│   ├── Module-by-Module Overview
│   ├── Consumption by Services
│   └── Dependencies & Policy
│
├── models.md                    # Shared data model
│   ├── InboxDB
│   ├── InboxStatus (lifecycle)
│   ├── InboxProgress
│   ├── InboxError
│   └── CreateInbox
│
├── infrastructure.md            # Shared utilities
│   ├── Storage Abstraction (InboxStore, MongoInboxStore)
│   ├── Error Handling (AppError, AppJson)
│   ├── Configuration & Logging
│   ├── Database Helpers
│   ├── Metrics
│   └── HTTP Middleware
│
├── auth.md                      # Authentication (feature `auth`)
│   ├── Loading Configuration (from_env_prefixed / from_lookup)
│   ├── Environment Variables ({PREFIX}_KEYCLOAK_*, ...)
│   ├── Derived URLs, Validation Rules
│   ├── build_auth_state, Role Checks
│   ├── Service-to-service tokens (ServiceTokenSource / ServiceTokenProvider)
│   └── Design Decision D10
│
├── testing.md                   # Test utilities
│   ├── Signed test tokens and mock Keycloak
│   ├── Builders
│   ├── Fixtures & Helpers
│   ├── Mock Store
│   ├── Unit Test Conventions
│   └── Integration Tests
│
└── mod.rs                       # Rust module documentation
```

## How to Use This Documentation

### For New Developers

1. Start with [`README.md`](README.md) for an overview of what `common` is
2. Read [`overview.md`](overview.md) to understand how it fits into the system
3. Read [`models.md`](models.md) to learn the shared inbox vocabulary
4. Refer to [`infrastructure.md`](infrastructure.md) when writing service code
5. Follow [`testing.md`](testing.md) when adding tests

### For Barbican/Citadel Contributors

1. Read [`models.md`](models.md) before touching inbox types
2. Read [`testing.md`](testing.md) to reuse the builders/fixtures/mock store
3. Update [`infrastructure.md`](infrastructure.md) if you change shared utilities

### For Architects

1. Read [`overview.md`](overview.md) for the ownership boundary and design rationale
2. Read [`models.md`](models.md) for the shared state vocabulary
3. Cross-reference with the service docs for the full picture

## Cross-References

### Related Topics

- **Inbox Lifecycle** → See [`models.md`](models.md#inboxstatus) and [`barbican/src/docs/state-machine.md`](../../barbican/src/docs/state-machine.md)
- **Storage Abstraction** → See [`infrastructure.md`](infrastructure.md#storage-abstraction) and [`overview.md`](overview.md#how-the-services-consume-common)
- **Error Handling** → See [`infrastructure.md`](infrastructure.md#error-handling)
- **Authentication / Keycloak env vars** → See [`auth.md`](auth.md)
- **Testing** → See [`testing.md`](testing.md) and the per-module `src/tests/` files

### Related Service Documentation

- **Barbican** → [`barbican/src/docs/README.md`](../../barbican/src/docs/README.md)
- **Citadel** → [`citadel/src/docs/README.md`](../../citadel/src/docs/README.md)

## Documentation Standards

### Markdown Format

- Use ATX-style headers (`#`, `##`, `###`)
- Include a table of contents for documents > 50 lines
- Use fenced code blocks with language specification
- Include relative links to other documentation files
- Add "See also" sections at the end of each document

### Code Examples

- Show complete, working examples when possible
- Include error handling
- Use realistic data values
- Annotate complex sections with comments

### Diagrams

- Use ASCII art for simple flow diagrams
- Include state transition diagrams
- Show component relationships
- Provide sequence diagrams for interactions

## Maintaining Documentation

### When to Update

- **Adding/changing a shared type** → Update [`models.md`](models.md)
- **Adding/changing a shared utility** → Update [`infrastructure.md`](infrastructure.md)
- **Changing auth config, JWT validation or auth middleware** → Update [`auth.md`](auth.md)
- **Changing the module layout** → Update [`overview.md`](overview.md) and [`README.md`](README.md)
- **Changing test utilities** → Update [`testing.md`](testing.md)
- **Adding a new file** → Update this file, `README.md`, and `mod.rs`

### Review Process

1. Make changes to the relevant documentation file
2. Update `README.md` if adding new files
3. Run `cargo doc --package common` to verify Rustdoc links
4. Test any code examples
5. Update cross-references if needed

## Generating Documentation

### Rust Documentation

```bash
# Generate HTML documentation
cargo doc --package common --no-deps

# Open in browser
cargo doc --package common --no-deps --open
```

### Markdown Documentation

The markdown files in this directory can be:
- Viewed directly in GitHub/GitLab
- Converted to HTML with pandoc
- Included in a larger documentation site

## Version Information

- **Documentation Version**: 1.0
- **Last Updated**: 2026-11-24
- **Common Version**: 0.0.0
- **Rust Edition**: 2024

---

*For questions or issues with documentation, please refer to the project's AGENTS.md or open an issue in the repository.*
