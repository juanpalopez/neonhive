# Security

neonhive reads agent transcripts from your disk. Those files can hold prompts,
source code and command output, so two properties matter:

- neonhive has no network code. It does not send data anywhere.
- neonhive opens session files read-only. It does not modify or delete them.

A bug that breaks either property is a security issue. So is a crash or hang
caused by a crafted transcript file.

## Reporting a vulnerability

Please do not open a public issue. Use
[GitHub private vulnerability reporting](https://github.com/juanpalopez/neonhive/security/advisories/new).

## Sharing snapshots

Frames made with `--snapshot` show prompts, file names and commands from the
session. Check them before you post them in an issue.
