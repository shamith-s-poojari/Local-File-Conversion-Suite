# pdf-protect

Adds AES-256 password protection, or removes it ([spec 13.6](../../docs/specs/phase-1.md#136-pdf-protect-pikepdf-two-modes)).

| | |
|---|---|
| **Tier** | E |
| **Engine** | pikepdf (qpdf), MPL-2.0 |
| **Encryption** | AES-256, security handler revision 6 |

## Passwords

Passwords arrive **only** in the request's `secrets`:

- `userPassword` and `ownerPassword` when protecting
- `password` when opening an encrypted input

The form collects them with the `password` widget, and the core moves them out of params. If a password does arrive in params, the adapter refuses the job. With no `ownerPassword`, a random one is generated, used once and never stored or shown.

## Modes

| Mode | Input | Rule |
|---|---|---|
| `protect` | Not encrypted | Needs `secrets.userPassword`. Encrypted input must be unlocked first |
| `unlock` | Open password | Needs the correct password. **No guessing, retries or cracking** |
| `unlock` | Permissions-only | Needs `confirmRights: true`, unless the owner password was given |
| `unlock` | Not encrypted | Refused as unnecessary |

## What the UI must say

- Permissions (print, copy, modify, annotate) are honored voluntarily by PDF viewers. They are not access control.
- A lost password can't be recovered.

## Known limits

The error catalog has no "invalid parameter" or "precondition" code. Validation therefore reports these as `E_INTERNAL`, with `param` and `detail.reason` set: `alreadyEncrypted`, `notEncrypted`, `confirmRights`, `passwordInParams`. The same applies to every adapter's parameter errors.
