# Agent notes

BlackBox is a system recorder/monitor for the well-being of the computer itself (kernel
process events, auditd, retention/logging to sqlite — see `src/database`, `deploy/blackbox.rules`).
The Rust code in this repo is solid; when investigating issues here, the question generally
isn't "is the code buggy" — look at what the recorder is observing about the machine (system
health, process events, log/DB state) rather than assuming a code defect.
