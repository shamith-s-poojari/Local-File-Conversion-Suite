# Contributing to Local File Conversion Suite

Thank you for your interest in contributing! This project benefits from contributions of all kinds — bug reports, feature suggestions, documentation improvements, new adapters, and code fixes.

## Code of Conduct

By participating in this project, you agree to maintain a respectful and constructive environment. Be kind, be patient, and assume good intentions.

## Reporting Bugs

**Found a bug?** Please [open a GitHub Issue](https://github.com/shamith-s-poojari/Local-File-Conversion-Suite/issues/new) with:

- A clear, descriptive title
- Steps to reproduce the problem
- What you expected to happen vs. what actually happened
- Your operating system and version
- The file format(s) involved (if applicable — do **not** attach files containing sensitive data)
- Any error codes or warning codes shown by the app

**Security vulnerabilities** should be reported privately — see [SECURITY.md](SECURITY.md).

## Suggesting Features

Feature ideas are welcome as GitHub Issues. Please include:

- A clear description of the problem you're trying to solve
- How you envision the solution working
- Whether you're willing to help implement it

## Development Setup

### Prerequisites

| Tool | Version | Install |
|---|---|---|
| Rust | See `rust-toolchain.toml` | [rustup.rs](https://rustup.rs/) |
| Node.js | 22 or newer | [nodejs.org](https://nodejs.org/) |
| pnpm | See `packageManager` in `package.json` | [pnpm.io](https://pnpm.io/) |
| uv | Latest | [docs.astral.sh/uv](https://docs.astral.sh/uv/) |

**macOS:** Install Xcode Command Line Tools (`xcode-select --install`).

### Getting Started

```bash
# Clone the repository
git clone https://github.com/shamith-s-poojari/Local-File-Conversion-Suite.git
cd Local-File-Conversion-Suite

# Install JavaScript dependencies
pnpm install

# Install Python dependencies
uv sync

# Start the development server
pnpm dev
```

### Running All Checks

Before submitting a pull request, run the full check suite:

```bash
pnpm check
```

This executes, in order:

1. Protocol schema validation (`pnpm --filter @converter/protocol check-generated`)
2. TypeScript type checking (`pnpm -r typecheck`)
3. JavaScript tests (`pnpm -r test`)
4. Rust formatting (`cargo fmt --all --check`)
5. Rust linting (`cargo clippy --workspace --all-targets -- -D warnings`)
6. Rust tests (`cargo test --workspace --no-fail-fast`)
7. Adapter registry validation (`registry-gen --check --include-dev`)
8. Python linting (`uv run ruff check .`)
9. Python tests (`uv run pytest`)

All checks must pass for a pull request to be merged.

## How to Contribute

### Adding a New Adapter (The Main Contribution Path)

Adapters are the most impactful way to contribute. Each adapter is a **self-contained Python module** — adding one requires **no changes to any shared file**. The registry is auto-generated from adapter folders at build time.

#### Adapter Structure

Create a new folder under `adapters/`:

```
adapters/your-adapter/
├── manifest.yaml        # Capabilities, edges, engine requirements
├── params.schema.json   # UI form is auto-generated from this schema
├── adapter.py           # The conversion/tool logic
├── tests/               # Adapter-specific tests
└── README.md            # What it does, known limits, engine, license
```

#### Key Rules

- The folder name **must** match the `id` in `manifest.yaml`
- The manifest is validated against `adapters/manifest.schema.json`
- Warning codes must be from the catalog in the protocol schema
- Required engines must be listed in `engines/engines.yaml`
- A **conversion** adapter declares `edges` in the format graph
- An **operation** adapter (merge, split, etc.) declares `inputs`, `outputs`, and `fidelity`
- Set `devOnly: true` for test/development adapters

#### Validate Your Adapter

```bash
pnpm registry
```

This regenerates the registry and validates all manifests. The conformance test kit in `workers/conformance/` provides standard tests.

For complete details, see the [Adapters README](adapters/README.md).

### Other Contributions

- **Rust core** (`crates/`): Bug fixes, performance improvements, new platform sandbox support
- **Frontend** (`apps/desktop/`): UI improvements, accessibility, new tool interfaces
- **CLI** (`apps/cli/`): New commands, output format improvements
- **Documentation** (`docs/`): Corrections, clarifications, new guides

## Code Style

### Rust

- Format with `cargo fmt`
- Lint with `cargo clippy -- -D warnings`
- All warnings are treated as errors in CI

### Python

- Lint with [Ruff](https://docs.astral.sh/ruff/)
- Follow the existing adapter patterns in `adapters/`

### TypeScript / JavaScript

- Follow the project `tsconfig.json` settings
- Type checking enforced via `pnpm typecheck`

## Commit Messages

This project uses [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>

[optional body]
```

**Types:**

| Type | Use For |
|---|---|
| `feat` | New feature or adapter |
| `fix` | Bug fix |
| `docs` | Documentation changes |
| `test` | Adding or updating tests |
| `refactor` | Code restructuring without behavior change |
| `perf` | Performance improvement |
| `chore` | Build, CI, or tooling changes |

**Examples:**

```
feat(adapter): add TIFF to PNG conversion adapter
fix(sandbox): handle symlinks in job directory on Linux
docs: clarify FFmpeg installation steps for Windows
test(sniffer): add HEIC magic byte detection tests
```

## Pull Request Process

1. **Fork** the repository and create a branch from `feat/app` (the active development branch)
2. **Make your changes** — keep each PR focused on a single concern
3. **Run `pnpm check`** and ensure all checks pass
4. **Write a clear PR description** explaining what changed and why
5. **Reference any related issues** using `Closes #123` or `Fixes #123`
6. **Be responsive** to review feedback

### What We Look For

- All CI checks pass
- Code follows the existing style and patterns
- New adapters include tests, a manifest, parameter schema, and a README
- Changes don't introduce network access or weaken sandbox isolation
- Documentation is updated if behavior changes

## License

By contributing to this project, you agree that your contributions will be licensed under the [GNU Affero General Public License v3.0 only](LICENSE) (AGPL-3.0-only).

---

**Questions?** Open a [GitHub Discussion](https://github.com/shamith-s-poojari/Local-File-Conversion-Suite/discussions) or comment on a relevant issue. We're happy to help!
