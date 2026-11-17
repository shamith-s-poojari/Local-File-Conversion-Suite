# Security policy

## Reporting a vulnerability

Please report it privately, using GitHub's private vulnerability reporting for this repository: open the **Security** tab, choose **Report a vulnerability**, and fill in the form. This creates a security advisory that only you and the maintainers can see.

Please don't open a public issue or pull request for a vulnerability, and don't post it elsewhere before it is fixed.

Helpful details:

- what you found and what an attacker gains (for example: a file that escapes the sandbox, a network connection, data left behind, a path that writes outside the chosen folder);
- the operating system and app version or commit;
- a file or steps that reproduce it. Don't send real personal documents.

## What to expect

This is a small project, so there are no promised response times. We will confirm that the report arrived, say whether we think it's a vulnerability, and tell you when a fix is out. If you'd like credit, tell us how you want to be named.

## Scope

In scope: anything that breaks a promise on the [privacy page](docs/privacy.md) or a mitigation in the [threat model](docs/threat-model.md), such as a sandbox escape, a network connection from the app or a worker, an unsandboxed parse of a user's file, a result that still holds data the tool says it removes, a write outside the chosen folder, or a flaw in a dependency that the app uses in a way that exposes it.

Known gaps are listed in the threat model and in [known limits](docs/known-limits.md). A report about one of them is welcome only if it shows more than the gap already says.

## Supported versions

There is no release yet. Only the latest commit on `main` is supported.
