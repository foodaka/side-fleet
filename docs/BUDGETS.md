# Agent budgets (spike)

A task can carry a budget, so its agent can pay for APIs that charge per call over the
[Machine Payments Protocol](https://mpp.dev) (HTTP 402) on [Tempo](https://tempo.xyz)'s
testnet. Payments under the task's threshold go through on their own; larger ones wait for
Approve on the page, like an "Ask me" tool call. This is a spike: testnet only, Claude Code and
Codex, behind `SIDEFLEET_PAYMENTS=1` on each machine.

## Who holds what

| Key | Where | Can |
|---|---|---|
| Your root key | A passkey on your phone or laptop (Settings, Wallet), or `~/.sidefleet/wallet.key` on your laptop (`sidefleet wallet new`) | Sign each task's access key |
| A task's access key | The worker process's memory only, made fresh for each run | Pay up to the task's budget, until it expires (24 h) |
| Public key, signed authorization, receipts, your wallet's address and passkey public key | The control plane (the wallet with your account's preferences) | Nothing: none of it can spend |

The limit is Tempo's: the root key signs a key authorization with an expiry and a spending limit
equal to the budget, and the chain refuses any transfer past it. Sidefleet also checks the price
against what is left before paying, and asks above the threshold, but nothing depends on that.

## How a run pays

1. `POST /v1/runs` with `budget_micros` (millionths of a dollar, at most $100) and optionally
   `ask_above_micros` (default $1).
2. The worker claims it, makes an access key and offers its address
   (`POST /v1/worker/runs/{id}/pay-key`): a `budget_key_ready` event, and a pending approval
   named `sidefleet_budget`, so the task waits in the bell and sends a push like any approval.
   Only a signature approves it; denying it ("Start without payments") starts the agent
   without the tool.
3. You sign the key for the budget: on the task's page with your passkey ("Authorize with
   passkey", which signs with viem and sends the authorization RLP-encoded), or on your laptop
   with `sidefleet wallet authorize <run id> --control <url>` (an API token with full access in
   `SIDEFLEET_API_TOKEN`). Either posts to `POST /v1/runs/{id}/pay-key/authorize`: a
   `budget_authorized` event, and the approval is approved.
4. The worker checks the authorization (this key, this chain, signed by the wallet, limit at
   most the budget) and starts the agent with a `pay` MCP tool (`sidefleet pay-mcp`) and a note
   about the budget: Claude Code in `--mcp-config`, allowed with `--allowedTools
   mcp__sidefleet_pay__pay`, the note in its system prompt; Codex in `-c mcp_servers.sidefleet_pay.*`
   for that run only, with `default_tools_approval_mode="approve"` (the tool asks the user itself)
   and a tool timeout past the approval wait, the note after the task. Nobody authorizing within the approval timeout starts the agent without the tool
   (`budget_skipped`).
5. The agent calls `pay` with a URL. The worker makes the request; on a 402 it reads the Tempo
   charge, refuses it if it is over what is left, asks on the page above the threshold (an
   approval with tool name `sidefleet_pay`), signs the payment with the access key, retries the
   request and returns the response. Each payment is reported
   (`POST /v1/worker/runs/{id}/payments`) as `payment_made` or `payment_refused`, and adds to the
   run's `spent_micros`.

## Approved services

Who the agent may pay without asking is your list of approved services (Settings, Approved
services; `GET`/`PUT /v1/pay-policy`), picked from the public MPP catalog
(`GET /v1/pay-services`, a cached copy of `https://mpp.dev/api/services`) or typed as a host.
A task with a budget gets a copy of the list when it is created (`runs.pay_policy`).

By default any service not on the list asks first, whatever the amount; the approval offers
"Pay and always allow", which adds it. "Pay any service" turns that off. Either way:

- A service is matched by the host the agent requested, which TLS proves; a service approved
  during a task stays approved for the rest of it.
- The first payment to an approved service pins the address it went to. A later payment that
  asks to go to another address asks first, so a swapped endpoint can't redirect money quietly.
- An amount over the task's threshold still asks.

The worker enforces this before it signs anything. Tempo itself only enforces the budget: the
catalog doesn't publish services' payment addresses, so the key can't be scoped to them.

## Try it

In Settings, Wallet, make a passkey wallet (it starts with test dollars from the faucet).
Passkeys need a secure page: https, or `localhost`. Or, on your laptop:

```sh
sidefleet wallet new     # a testnet key in ~/.sidefleet/wallet.key
sidefleet wallet fund    # test dollars from the faucet
sidefleet wallet show
```

On a worker machine, add `SIDEFLEET_PAYMENTS=1` to `~/.sidefleet/worker.env` and restart the
worker. Start a Claude Code task with a budget (task options, Budget) that needs a paid API,
for example "Call https://mpp.dev/api/ping/paid with the pay tool and tell me what it says"
($0.10), then authorize it on the task's page (or run the command it shows).

`cargo test -p sidefleet-worker --lib -- --ignored tempo_refuses` checks against the testnet
that a key authorized for $0.05 can't pay a $0.10 charge, with `SIDEFLEET_TEST_WALLET_KEY`
set to a funded wallet's key file; `-- --ignored a_passkey_wallet` checks that a passkey
wallet's authorization pays on chain (`SIDEFLEET_TEST_PASSKEY_AUTH`, see the test).

## Not done yet

- A passkey made on one site works only there (WebAuthn binds it to the domain), so a wallet
  made on a self-hosted control plane can't sign on the hosted one.
- opencode: 2.x reads MCP servers only from a project config file (not the environment, nor a
  config file or folder named in it), which would have to go in the task's own worktree. A task
  with opencode can't have a budget yet.
- Call scopes on the access key (TIP-1011), so it can only call the token's transfer functions,
  and only to pinned addresses.
- Choosing services per task in the composer; today a task takes your whole list.
- Mainnet, other tokens than pathUSD, MPP sessions (pay-as-you-go) and card payments.
- A budget across a whole thread: today each message of a thread gets its own budget of the
  same size, and a run moved to another machine loses its budget.
