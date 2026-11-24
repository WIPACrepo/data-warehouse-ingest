# Documentation Structure

This directory contains comprehensive documentation for the Barbican crate, organized by topic for easy navigation.

## File Organization

### Core Documentation

| File | Size | Purpose |
|------|------|---------|
| [`README.md`](README.md) | 5.9 KB | Main index with quick start guide and overview |
| [`architecture.md`](architecture.md) | 14 KB | System architecture, component diagram, module structure |
| [`api-reference.md`](api-reference.md) | 7.4 KB | Complete HTTP API endpoint documentation |
| [`state-machine.md`](state-machine.md) | 12 KB | Inbox lifecycle, states, and transition rules |
| [`authentication.md`](authentication.md) | 15 KB | JWT validation, roles, and security |
| [`citadel-integration.md`](citadel-integration.md) | 16 KB | Backend service coordination and triggers |
| [`configuration.md`](configuration.md) | 15 KB | Environment variables and settings |
| [`development.md`](development.md) | 15 KB | Building, testing, and extending Barbican |

### Design Documents

| File | Size | Purpose |
|------|------|---------|
| [`01-design-plan.md`](01-design-plan.md) | 30 KB | Complete architecture and API specification |
| [`02-completion-report.md`](02-completion-report.md) | 7.9 KB | Implementation status and verification |

### Module Files

| File | Purpose |
|------|---------|
| [`mod.rs`](mod.rs) | Rust module documentation |
| [`STRUCTURE.md`](STRUCTURE.md) | This file - documentation organization |

## Documentation Hierarchy

```
barbican/src/docs/
│
├── README.md                    # Main entry point
│   ├── Quick Start
│   ├── Architecture Overview
│   ├── Module Structure
│   └── Links to all other docs
│
├── architecture.md              # System design
│   ├── High-Level Architecture
│   ├── Component Flow
│   ├── Module Structure
│   ├── Data Flow
│   └── Key Design Decisions
│
├── api-reference.md             # API documentation
│   ├── External Endpoints
│   ├── Internal Endpoints
│   ├── Authentication
│   ├── Health Endpoints (/health, /ready)
│   ├── Error Responses
│   └── Rate Limiting
│
├── state-machine.md             # Inbox lifecycle
│   ├── State Descriptions
│   ├── Transition Diagram
│   ├── Action-Based Transitions
│   └── Optimistic Locking
│
├── authentication.md            # Security
│   ├── JWT Authentication
│   ├── Token Validation
│   ├── JWKS Management
│   ├── Role-Based Access Control
│   └── Service Accounts
│
├── citadel-integration.md       # Backend coordination
│   ├── Automatic Triggers
│   ├── Progress Reporting
│   ├── Service-to-Service Communication
│   └── Error Handling
│
├── configuration.md             # Settings
│   ├── Required Configuration
│   ├── Environment Variables (service: Config; auth: AuthConfig, D10)
│   ├── Example Configurations
│   └── Validation Rules
│
├── development.md               # Developer guide
│   ├── Building & Running
│   ├── Testing
│   ├── Code Style
│   ├── Adding Features
│   └── Debugging
│
├── 01-design-plan.md            # Original design specification
├── 02-completion-report.md      # Implementation status
└── mod.rs                       # Rust module documentation
```

## How to Use This Documentation

### For New Developers

1. Start with [`README.md`](README.md) for overview
2. Read [`architecture.md`](architecture.md) to understand system design
3. Follow [`development.md`](development.md) to set up your environment
4. Reference [`api-reference.md`](api-reference.md) for endpoint details

### For API Consumers

1. Review [`api-reference.md`](api-reference.md) for endpoint documentation
2. Understand [`authentication.md`](authentication.md) for JWT requirements
3. Study [`state-machine.md`](state-machine.md) for inbox lifecycle

### For Operators

1. Consult [`configuration.md`](configuration.md) for environment setup
2. Review [`citadel-integration.md`](citadel-integration.md) for service coordination
3. Reference [`development.md`](development.md) for deployment guidance

### For Architects

1. Read [`architecture.md`](architecture.md) for system design
2. Review [`01-design-plan.md`](01-design-plan.md) for original specifications
3. Study [`state-machine.md`](state-machine.md) for lifecycle design

## Cross-References

### Related Topics

- **Authentication** → See [`authentication.md`](authentication.md) and [`api-reference.md`](api-reference.md#authentication)
- **State Transitions** → See [`state-machine.md`](state-machine.md) and [`api-reference.md`](api-reference.md#internal-api-endpoints)
- **Citadel Integration** → See [`citadel-integration.md`](citadel-integration.md) and [`architecture.md`](architecture.md#component-flow)
- **Configuration** → See [`configuration.md`](configuration.md) and [`development.md`](development.md#running-locally)

### Design Documents

- **Original Plan** → [`01-design-plan.md`](01-design-plan.md)
- **Implementation Status** → [`02-completion-report.md`](02-completion-report.md)
- **Architecture Decisions** → [`architecture.md`](architecture.md#key-design-decisions)

## Documentation Standards

### Markdown Format

- Use ATX-style headers (`#`, `##`, `###`)
- Include table of contents for documents > 50 lines
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

- Adding new endpoints → Update [`api-reference.md`](api-reference.md)
- Changing configuration → Update [`configuration.md`](configuration.md)
- modify state machine → Update [`state-machine.md`](](api-reference.md)
- Modifying state machine.md)
- Adding features → Update [`development.md`](development.md) and relevant docs
- Changing architecture → Update [`architecture.md`](architecture.md)

### Review Process

1. Make changes to relevant documentation file
2. Update README.md if adding new files
3. Run `cargo doc --package barbican` to verify Rustdoc links
4. Test any code examples
5. Update cross-re references if needed

## Generating Documentation

### Rust Documentation

```bash
# Generate HTML documentation
cargo doc --package barbican --no-deps

# Open in browser
cargo doc --package barbican --no-deps --open
```

### Markdown Documentation

The markdown files in this directory can be:
- Viewed directly in GitHub/GitLab
- Converted to HTML with pandoc
- Included in a larger documentation site

## Version Information

- **Documentation Version**: 1.0
- **Last Updated**: 2026-08-17
- **Barbican Version**: 0.0.0
- **Rust Edition**: 2024

---

*For questions or issues with documentation, please refer to the project's CONTRIBUTING.md or open an issue in the repository.*
