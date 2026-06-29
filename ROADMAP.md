# AxiomLab Production Roadmap

This is the authoritative plan for moving AxiomLab from a credible virtual-lab
alpha to a deployable multi-operator system. Current capabilities are described
in `README.md`; operating details are in `OPERATOR_GUIDE.md`.

## Current baseline

AxiomLab already provides the complete safety path: LLM proposal, ordered
fail-closed gates, exact-scope operator approval, simulator or SiLA execution,
and signed audit. Directive and approval lifecycle journals survive process
restarts. Deterministic simulator faults, benchmark scenarios, Rust integration
tests, and Chromium operator tests provide a hardware-free validation loop.

The remaining work is mainly production systems engineering. JSON journals are
single-process durability, JWT is submission-only authentication, protocols are
free-form directives rather than versioned resources, and chemistry is a policy
table rather than a validated scientific model.

## Delivery order

### Phase 1 — Identity and authorization

**Goal:** every read, mutation, approval, and audit export has an authenticated
human or service identity and an explicit permission decision.

- Add OIDC Authorization Code + PKCE login and secure HTTP-only sessions.
- Define roles: `viewer`, `operator`, `approver`, `administrator`, and
  `service`, with route-level permissions.
- Derive approver identity from the session; never accept `approver_id` from a
  request body.
- Add CSRF protection, session expiry/revocation, security headers, and an
  open-development-mode banner.
- Record subject, role, session id, and authorization result in the audit chain.

**Acceptance:** anonymous mutations fail; operators cannot approve their own run
when separation-of-duties policy is enabled; revoked sessions stop working; API
and browser tests cover each role boundary.

### Phase 2 — Transactional operational state

**Goal:** replace JSON projections with crash-safe, queryable state while keeping
the signed audit chain as the evidence record.

- Introduce a repository layer and SQLite first; retain a Postgres-compatible
  schema for later multi-node deployment.
- Store directives, run attempts, approval records, inventory projections,
  calibrations, sessions, and worker leases transactionally.
- Use idempotency keys and optimistic versions on every mutation.
- Claim work using leases; expired leases become `recovery_required`, not
  automatically re-executed.
- Add migrations, backup/restore tests, and rebuild projections from audit.

**Acceptance:** kill the process at every run transition and recover without
duplicate actuation; two workers cannot claim the same run; a restored database
matches the signed audit history.

### Phase 3 — Versioned protocol engine and recovery

**Goal:** make execution reproducible and operator-controllable rather than
depending only on a free-form directive.

- Define immutable protocol versions with typed steps, parameters, resources,
  expected observations, and safety-policy version.
- Compile a directive into a draft protocol; require review before execution.
- Add run states for pause, resume, cancel, emergency stop, manual takeover,
  reconciliation, and compensation.
- Persist step checkpoints and instrument command ids.
- Require observation or reconciliation after uncertain/partial execution.

**Acceptance:** identical protocol versions produce identical plans; restart
continues only from a proven checkpoint; partial dispense blocks subsequent
steps until an operator records the observed state.

### Phase 4 — Virtual-lab depth and safety evidence

**Goal:** expose recovery defects before any hardware pilot.

- Add contamination, carryover, depletion, calibration drift, thermal lag,
  delayed responses, duplicate acknowledgements, and out-of-order events.
- Seed all stochastic models and publish scenario fixtures.
- Expand benchmark coverage across every gate and run-state transition.
- Add adversarial LLM proposals and property/state-machine tests.
- Version chemistry policy sources, units, confidence, and review status.

**Acceptance:** every declared failure mode has a deterministic regression;
unsafe proposals are rejected before execution; uncertain execution always
enters reconciliation; validation reports are generated in CI.

### Phase 5 — Deployment and operations

**Goal:** make a single-node deployment supportable before scaling it.

- Build pinned containers, production configuration validation, and secret
  injection; never persist secrets in UI storage.
- Add structured tracing, run/approval latency metrics, alerts, and dashboards.
- Document key rotation, audit export, backup/restore, incident response, and
  disaster recovery.
- Add dependency, container, and secret scanning plus an SBOM.
- Define retention and privacy policy for directives, identities, and audit data.

**Acceptance:** a clean environment can deploy from documented commands;
backup restoration and signing-key rotation are rehearsed; alerts fire during
injected failures; the operator runbook covers degraded operation.

### Phase 6 — Optional hardware pilot

**Goal:** validate one narrow adapter and protocol with a partner lab or borrowed
device after the software recovery model is stable.

- Select one SiLA 2 instrument and one low-risk protocol.
- Add conformance tests for capabilities, units, timeout semantics, command ids,
  and cancellation behavior.
- Run dry, supervised, and failure-injection qualification stages.
- Record discrepancies between simulator assumptions and device behavior.

**Acceptance:** the adapter passes a documented qualification matrix and the
pilot can be stopped, reconciled, and audited without relying on the LLM.

## Recommended next milestone

Implement Phases 1 and 2 together as a **single-node trusted-operator release**:
OIDC sessions, RBAC, SQLite repositories, worker leases, migrations, and
crash-point integration tests. Do not add more instrument types until this
foundation is complete.

## Deliberate non-goals

- Autonomous scientific discovery or unrestricted tool creation.
- Claiming formal verification for chemistry, distributed systems, or external
  instruments.
- Multi-region or high-availability deployment before single-node recovery is
  demonstrated.
- Automatic replay of an action whose physical outcome is uncertain.
