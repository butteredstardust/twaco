# Security policy

## Report a vulnerability

Report vulnerabilities privately, through
[GitHub's private vulnerability reporting](https://github.com/butteredstardust/twaco/security/advisories/new).
Do not open a public issue.

Include the steps to reproduce it, the version (`twaco --version`), and what an attacker
could do. Leave out real credentials and hostnames.

## What is in scope

twaco handles server credentials and writes to servers and to the files of a solution, so
these matter most:

- a credential printed, logged, written to a file, or passed to a process that did not ask
  for it;
- a write to a server without `--apply` (CLI) or `dry_run: false` (MCP), other than `call`
  on the command line, which runs a service by design;
- a file written outside the solution or the folder a command was given, including through
  links or crafted entity names;
- a crafted entity file, server response or archive that makes twaco crash, hang, or write
  something it should not;
- an MCP argument that escapes the checks above.

The security of a ThingWorx server itself is PTC's; report platform issues to PTC.

## Response

We aim to acknowledge a report within 10 business days and to agree a disclosure timeline
with you. A fix is released before the details are made public, and reporters are credited
unless they prefer not to be.

## Supported versions

Security fixes are made to the latest release.

> twaco is licensed under the MIT License and provided "as is", without warranty of any kind.
> See [LICENSE](LICENSE).
