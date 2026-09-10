# Axiom UI primitives

Small React presentation components backed by Axiom's shared CSS tokens:
`Surface`, `Notice`, `Spinner`, and `ConfirmDialog`. Import `@axiom/ui/styles.css`
after `@axiom/brand/tokens.css`. Hosted auth consumes these directly.

Confirmation dialogs use native modal dialogs, focus Cancel initially, restore
focus to the initiating control, trap keyboard focus through the browser, and
prevent accidental dismissal during an in-flight mutation. Provider and account
logic belongs to the application, not this package.
