# Documentation Structure

This directory contains comprehensive documentation for the Citadel crate, organized by topic for easy navigation.

## File Organization

### Core Documentation

| File | Size | Purpose |
|------|------|---------|
| [`README.md`](README.md) | ~9.3 KB | Main index with quick start guide and overview |
| [`architecture.md`](architecture.md) | ~30.5 KB | System architecture, component diagram, module structure |
| [`api-reference.md`](api-reference.md) | ~20.8 KB | Complete HTTP API endpoint documentation |
| [`state-machine.md`](state-machine.md) | ~16.0 KB | Job lifecycle, statuses, and inbox state coordination |
| [`authentication.md`](authentication.md) | ~23.2 KB | JWT validation, roles, job-route security, Barbican service token |
| [`barbican-integration.md`](barbican-integration.md) | ~18.4 KB | Frontend service coordination |
| [`configuration.md`](configuration.md) | ~26.1 KB | Environment variables and settings |
| [`development.md`](development.md) | ~16.1 KB | Building, testing, and extending Citadel |
| [`recursive-transfer.md`](recursive-transfer.md) | ~22.4 KB | Recursive transfer engine, symlink safety, atomic commit, error collection |
| [`symlink-limitations.md`](symlink-limitations.md) | ~9.5 KB | Symlink edge cases, bind mounts, TOCTOU, accepted risks |
| [`security-considerations.md`](security-considerations.md) | ~18.7 KB | Threat model, hardening guide, incident response, compliance |

### Module Files

| File | Purpose |
|------|---------|
| [`mod.rs`](mod.rs) | Rust module documentation |
| [`STRUCTURE.md`](STRUCTURE.md) | This file - documentation organization |

## Documentation Hierarchy

```
citadel/src/docs/
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
│   ├── Inbox Endpoints
│   ├── Job Endpoints
│   ├── Callback Endpoint
│   ├── Health Endpoints
│   ├── Error Responses
│   └── Authorization
│
├── state-machine.md             # Job lifecycle
│   ├── Job Statuses
│   ├── Lifecycle Diagram
│   ├── Job Type Checklist
│   ├── Inbox State Coordination
│   └── Failure Handling
│
├── authentication.md            # Security
│   ├── JWT Authentication
│   ├── Token Validation
│   ├── JWKS Management
│   ├── Role-Based Access Control
│   ├── Job Route Security (admin JWT or callback secret, D11)
│   ├── Outbound: Citadel → Barbican Service Token
│   └── Security Considerations
│
├── barbican-integration.md      # Frontend coordination
│   ├── Coordination Model
│   ├── Barbican API Client
│   ├── Authentication to Barbican
│   ├── Retry Strategy
│   ├── Inbox State Ownership (incl. Compensation)
│   └── Error Handling
│
├── configuration.md             # Settings
│   ├── Required Configuration
│   ├── Environment Variables
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
├── recursive-transfer.md        # Transfer engine
│   ├── Symlink Handling
│   ├── Loop Detection
│   ├── Transfer Pipeline
│   ├── Atomic Transfer Pattern
│   ├── Error Collection & Cancellation
│   └── Configuration Examples
│
├── symlink-limitations.md       # Symlink edge cases
│   ├── Platform Specificity
│   ├── Bind Mount Detection
│   ├── TOCTOU Vulnerabilities
│   ├── Symlink Farm Attacks
│   └── Other Edge Cases
│
├── security-considerations.md   # Threat model & hardening
│   ├── Threat Model
│   ├── Symlink Attack Vectors
│   ├── Mitigations
│   ├── Configuration Hardening Guide
│   ├── Observability & Audit Logging
│   └── Incident Response
│
└── mod.rs                       # Rust module documentation
```

## How to Use This Documentation

### For New Developers

1. Start with [`README.md`](README.md) for overview
2. Read [`architecture.md`](architecture.md) to understand system design
3. Read [`state-machine.md`](state-machine.md) to understand the job lifecycle
4. Follow [`development.md`](development.md) to set up your environment
5. Reference [`api-reference.md`](api-reference.md) for endpoint details

### For API Consumers / Integrators

1. Review [`api-reference.md`](api-reference.md) for endpoint documentation
2. Understand [`authentication.md`](authentication.md) for JWT requirements
3. Review [`barbican-integration.md`](barbican-integration.md) for service coordination
4. Study [`state-machine.md`](state-machine.md) for job and inbox lifecycles

### For Operators

1. Consult [`configuration.md`](configuration.md) for environment setup
2. Review [`barbican-integration.md`](barbican-integration.md) for service coordination
3. Read [`state-machine.md`](state-machine.md) for operational state handling
4. Reference [`development.md`](development.md) for deployment guidance

### For Architects

1. Read [`architecture.md`](architecture.md) for system design
2. Study the key design decisions in [`architecture.md#key-design-decisions`](architecture.md#key-design-decisions)
3. Study [`state-machine.md`](state-machine.md) for lifecycle design

## Cross-References

### Related Topics

- **Authentication** → See [`authentication.md`](authentication.md) and [`api-reference.md`](api-reference.md#authentication)
- **Job Lifecycle** → See [`state-machine.md`](state-machine.md) and [`api-reference.md`](api-reference.md#job-endpoints)
- **Barbican Coordination** → See [`barbican-integration.md`](barbican-integration.md) and [`architecture.md`](architecture.md#component-flow)
- **Configuration** → See [`configuration.md`](configuration.md) and [`development.md`](development.md#running-locally)
- **Recursive Transfer** → See [`recursive-transfer.md`](recursive-transfer.md) and [`architecture.md`](architecture.md#key-design-decisions)
- **Symlink Security** → See [`symlink-limitations.md`](symlink-limitations.md) and [`security-considerations.md`](security-considerations.md)
- **Barbican's view of this integration** → [`barbican/src/docs/citadel-integration.md`](../../barbican/src/docs/citadel-integration.md)
- **Architecture Decisions** → [`architecture.md`](architecture.md#key-design-decisions)

## Documentation Standards

### Markdown Format

- Use ATX-style headers (`#`, `##`, `###`)
- Include table of contents for documents > 50 lines
- Use fenced code blocks with language specification
- Include relative links to other documentation files
- Add "See also" sections at the end of each document
- Use **"Planned"** callouts to distinguish planned behavior from the current implementation

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
- Modifying job lifecycle → Update [`state-machine.md`](state-machine.md)
- Adding features → Update [`development.md`](development.md) and relevant docs
- Changing architecture → Update [`architecture.md`](architecture.md)
- Changing the transfer engine or symlink handling → Update [`recursive-transfer.md`](recursive-transfer.md), [`symlink-limitations.md`](symlink-limitations.md), and [`security-considerations.md`](security-considerations.md)
- Changing route authentication (`create_app`, `middleware/job_auth.rs`) or the Barbican service token → Update [`authentication.md`](authentication.md), [`api-reference.md`](api-reference.md), and [`configuration.md`](configuration.md)
- Changing compensation (`handlers/compensate.rs`) → Update [`barbican-integration.md`](barbican-integration.md#compensation) and [`state-machine.md`](state-machine.md)
- Changing security posture / hardening defaults → Update [`security-considerations.md`](security-considerations.md) and [`configuration.md`](configuration.md)

### Review Process

1. Make changes to relevant documentation file
2. Update `README.md` if adding new files
3. Run `cargo doc --package citadel` to verify Rustdoc links
4. Test any code examples
5. Update cross-references if needed

## Generating Documentation

### Rust Documentation

```bash
# Generate HTML documentation
cargo doc --package citadel --no-deps

# Open in browser
cargo doc --package citadel --no-deps --open
```

### Markdown Documentation

The markdown files in this directory can be:
- Viewed directly in GitHub/GitLab
- Converted to HTML with pandoc
- Included in a larger documentation site

## Version Information

- **Documentation Version**: 1.1
- **Last Updated**: 2026-08-26
- **Citadel Version**: 0.0.0
- **Rust Edition**: 2024

---

*For questions or issues with documentation, please refer to the project's AGENTS.md or open an issue in the repository.*
