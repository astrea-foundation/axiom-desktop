# Repository ownership

This repository owns Axiom Desktop, AxiomCLI, the local proxy, and installer
builds. All hosted website and account UI, release catalogs, web assets and
Worker deployment tools belong exclusively in the companion backend repository,
`axiom`, under `landing/` and `auth/`. Do not add hosted web workspaces here.

Native branding and the compiled local loopback callback page remain in
`packages/brand/`. Keep this repository independently buildable. Desktop consumes
the public release API; update tests use native fixtures, never a neighboring
website checkout. Installer files and their generated manifest are the handoff
to the backend repository's CDN and website publisher.
