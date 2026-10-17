# Performance report

Performance depends on the hardware, operating-system state, and workload. Exact workstation details and machine-specific measurements are omitted from this public report.

The workspace includes `scripts/measure-performance.ps1` for collecting a local idle sample. It starts and closes the release app and server, then removes its temporary database. Run it from the project root with PowerShell when you want a measurement for your own machine.
