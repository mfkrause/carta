# Security Policy

carta is a document converter: it ingests arbitrary, potentially untrusted input and
renders it to another format. Robustness against malformed input is a core goal — a panic,
hang, or unbounded allocation on hostile input is treated as a security-relevant bug, not
just a correctness one.

## Supported versions

carta is in early alpha. Security fixes are applied to the latest release and the `main`
branch only. There is no long-term-support or back-porting commitment yet.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Report privately through either channel:

- **GitHub private vulnerability reporting** — on the repository, go to the **Security**
  tab and choose **Report a vulnerability** (preferred; keeps the report and discussion
  in one place).
- **Email** — [max@kuatsu.de](mailto:max@kuatsu.de). If you would like an encrypted
  channel, say so in a first message and we will arrange one.

Please include enough detail to reproduce: the input (or a minimal reproducer), the exact
command or API call, the versions involved, and the observed vs. expected behavior. If the
issue is a crash or hang, a minimized reproducing input is enormously helpful.

## What to expect

- We aim to acknowledge a report within a few days.
- We will confirm the issue, keep you updated on progress, and credit you in the release
  notes once a fix ships — unless you prefer to remain anonymous.
- Please give us a reasonable window to release a fix before any public disclosure.
