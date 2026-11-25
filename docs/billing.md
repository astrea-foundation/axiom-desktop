# Credits and subscription billing

The development client supports weekly subscription credits, persistent purchased
credit in USD, card funding, crypto funding and explicit automatic card top-up. The
server advertises each available capability; this source does not establish a
live subscription offering.

Open **Account menu → Billing**. The **Balance** at the top is in dollars: free
trial credit plus **Added credit** (purchases and gift codes), with the time it
was last updated. Weekly credits show their next reset and expire without
rollover.
Weekly balances display whole credits while usage retains fractions; a positive
remainder below one displays as **Less than 1**. Money uses fixed precision of
100,000,000 units per dollar; a positive balance below one cent displays as
**Less than $0.01**. Held amounts and usage retain exact fractions.
The default credit value is $0.001 (0.1 cent): $1 of reference API usage consumes
1,000 credits at the default multiplier. The $1 lifetime trial opens as 1,000
credits. Weekly defaults are 1,250/5,000/12,500/50,000 credits for unchanged
$2.50/$10/$25/$100 monthly payments. Plans and credit values come from the server;
the client keeps their exact amounts and does not assume a one-cent credit.
Plans also show monthly totals of 5,000/20,000/50,000/200,000 credits. Plan names,
prices, allowance lines and action buttons align across the cards. One timing
note below the plans explains that totals use four weekly grants; the basis
is not repeated after each amount. Current plan details and confirmations use
the same totals. Credits still refresh every seven days, and a calendar month
can include five grants.
While a request is running, its reservation can reduce available dollars below
the posted balance. Billing refreshes and gift redemption still report the valid
available balance during these holds.
Opening Billing shows eligible legacy balances without enrolling the account in
credits. One-time card top-ups retain the account's existing spending authority.
Eligible trial credits are separate. The spending order is weekly, eligible trial,
then purchased dollars when fallback is enabled. The two balances are never summed.

Free trials share a configurable budget across all accounts and providers over
the preceding five hours. Once it is full, new trial-funded requests show **Free
trials are currently at capacity** and a check-back time in the computer's own
time zone. In Desktop the notice includes **Upgrade plan**, which opens Balance to
choose a plan or add credit and keep chatting. Existing requests finish
normally; their cost can take the shared spending over the limit and move the
retry time later. Subscription allowance and purchased dollars remain usable
when available and do not contribute to the trial budget. Purchased fallback
still follows your account setting. This limit does not reset your lifetime trial
grant or turn it into a recurring allowance.

Plans are billed on the calendar-month billing anniversary while allowance
refreshes every seven days. A month can include five weekly grants. Changing
plans schedules the new price at the next renewal. **Manage subscription** opens
Stripe's reviewed payment-method/cancellation portal. Cancelling retains paid
service until its end. A failed renewal does not grant unpaid allowance.
After a subscription is canceled or expires, choose Subscribe to start a fresh
Checkout. Pending subscriptions retain their original operation for resumption.

Choose **Pay from balance** in the plan confirmation to subscribe with added
credit. Review the price, weekly credits, first renewal date and remaining balance
before authorizing the recurring payment. Every month the plan renews at the
agreed price using added credit. If the balance is too low, the plan ends.
**Cancel renewal** stops future debits and keeps already-paid benefits. Use
**Resume renewal** before coverage ends to turn it back on. Balance payments work
while card processing is unavailable, provided any existing card subscription
or submitted checkout has been resolved.

Added credit spends at actual reference API cost: $10 covers $10 of API usage,
independent of model multipliers or changes to subscription-credit value.
Subscription credits default to $0.01 of reference API usage at a 1× multiplier.
Trial grants retain their recorded conversion when subscription pricing changes.
Starter/Standard/Plus/Max cost $2.50/$10/$25/$100
per month and grant 125/500/1,250/5,000 credits each week. Four fully used weeks
cover twice the plan price in API cost; these are weekly benefits rather than
fixed calendar-month totals. The server catalog and model-pricing policy are
authoritative.

**Card** opens hosted Stripe Checkout on `checkout.stripe.com`, which supports
both `/c/pay/` and `/g/pay/` links. **Crypto** offers the configured currencies
and a separate direct Zcash deposit option. Money paid is displayed in USD;
confirmed funding adds the server-verified dollar value to purchased credit. Payment return
links do not prove payment: the server reconciles the original owned record.
If Stripe cannot process a billing request, Desktop displays “The credit card
processor is temporarily unavailable.”
Retry an uncertain operation under the same ID. Card details and billing
credentials stay outside the renderer and conversation.

Saving a card or subscribing leaves automatic top-up off. Enable it only after
choosing the dollar-balance threshold, refill amount, UTC calendar-month cap and saved
card, and accepting the displayed authorization. Refills become eligible when
weekly allowance is exhausted and purchased availability is below the threshold.
Pending/unknown attempts count against the cap, and refunds do not replenish
it. Subscription charges are separate. Turning off prevents new dispatch;
already submitted payments still reconcile. An authentication-required attempt
can be cancelled before replacing its card/authorization.

Billing’s **Usage** section shows the credits and purchased dollars used by settled
requests. It refreshes about every 30 seconds while visible and when the window
regains focus, even if card history is unavailable. A failed usage refresh displays
an error in the section.
For accounts included in a credit denomination reset, totals start at that reset.

The Usage screen separates weekly allowance usage from purchased dollars, with
exact per-model totals. It excludes trial spending from the allowance chart.
When an older API returns mixed trial and allowance rows, Desktop displays the
dollar report until the API can supply the separate figures.
Accounts without weekly plan credits see dollar spending instead. API key reference
cost is separate from credits consumed or cash payments. Week/month reporting
uses the local timezone and calendar boundaries; allowance resets use the
subscription's seven-day schedule.

## CLI and TUI

Account controls require interactive native sign-in. `AXIOM_API_KEY` cannot
authorize these operations. Outputs are bounded, validated JSON. Card/portal
URLs are returned for explicit browser opening; they are restricted to approved
Stripe hosts. Customer Portal session URLs use `/p/session?secret=…` or
`/p/session/…` on `https://billing.stripe.com`.

```sh
axiomcli billing status
axiomcli billing plans
axiomcli billing prices
axiomcli billing usage --period month --timezone America/Los_Angeles
axiomcli billing subscribe --id UUID_V4 --plan starter --catalog-version plans-v3
axiomcli billing change-plan --id UUID_V4 --plan standard --catalog-version plans-v3
axiomcli billing top-up --id UUID_V4 --amount-cents 1000
axiomcli billing save-card --id UUID_V4
axiomcli billing cards
axiomcli billing portal
axiomcli billing auto-topup
axiomcli billing enable-auto-topup --threshold-usd 1.00 \
  --amount-cents 1000 --monthly-cap-cents 3000 --payment-method pm_CARD --consent
axiomcli billing disable-auto-topup
axiomcli billing fallback off
axiomcli billing operations
axiomcli billing check --id OPERATION_UUID
```

Generate a UUID v4 for each new intentional payment/change and retain it to retry
that operation. Use `--help` for owned payment checks, cancellation and other
controls. The TUI `/balance` overlay displays weekly credits and purchased dollars separately
and retains private gift-code redemption and direct deposit details.

## Integration

ACP `billing@7` carries `_axiom/billing/credits` and `credits@3` with a finite tagged action/result
contract. The native client chooses static API paths and uses a short native
access-token lease. It validates exact credit strings, allocations, dates and
hosted destinations before crossing ACP. Account/runtime generations fence
late replies at the native, SDK, IPC and renderer boundaries. Legacy monetary
contracts are retained; credits are not relabeled as `paidMicrousd`.

`billing@9` adds consented balance-plan purchases and renewal controls to the
same contract. Desktop shows these controls when the sidecar and server
advertise automatic balance renewal.

Older sidecars continue basic funding but need an update for subscription/card
controls. A 404 summary means the server has not enrolled that account; the UI
retains legacy balances. Release qualification still requires the selected
sandbox's real Checkout/SCA/webhook/renewal flows and the promoted installers.
