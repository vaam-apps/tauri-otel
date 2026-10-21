# Export floor and breadcrumbs

Every log record, from Rust or from the page, passes through one floor
(`WARN` by default, set with
[`export_floor`](/guide/configuration#export-floor-level-and-breadcrumbs-n)).

- A record **at or above** the floor is exported.
- A record **below** it is **withheld** in a short ring (12 records by default,
  set with `breadcrumbs`), and sent only if an `ERROR` follows while it is
  still there.
- It then goes as one extra `breadcrumbs` record, emitted just before the
  fault, carrying lines like:

```text
-0.412s INFO webview: listing editor opened
```

Each line is the time before the fault, the level, the target (`webview` for
the page's records) and the message.

## The rules

- **A `WARN` gets no trail.** An expected condition is not something anyone
  opens a stack trace for.
- **A trail is spent once it is sent.** A second error a moment later does not
  resend the first one's context.
- **With nothing withheld, there is no trail.** It would duplicate records
  already sent.
- **A withheld record is never exported on its own later.** It rides with a
  fault or it never leaves.
- **Events inside a span are held to the same floor.** `tracing-opentelemetry`
  turns them into span events, and without the floor every `debug!` in a
  command would reach the collector through the trace instead.
- **One `WARN` per launch** names the service, version, environment, endpoint
  and floor. At the default floor an `INFO` would be withheld, and a quiet
  build could not tell you why it is quiet.

One ring serves both halves of the app, which is why the page's records go
through the Rust side rather than through a second logger: the trail of an
error in a Rust command includes what the page was doing just before it.

## Designing for it

- **Log at `INFO` and `DEBUG` freely.** It costs nothing unless something
  fails, and then it is the context the error needed.
- **Name your crates in `filter`.** An event the
  [filter](/guide/configuration#filter-directives) drops never reaches the
  ring, so it can never be a breadcrumb. `info,my_app=debug` keeps your own
  `DEBUG` context.
- **Use `WARN` for conditions an operator should see without a failure.** A
  `WARN` carries no breadcrumb trail.
- **`breadcrumbs(0)` turns the trail off.** Records below the floor are then
  simply dropped.

## Why

The design comes from a mobile client in this organisation that measured its
own stream: 96.5% of what it shipped was debug and info chatter, one record per
navigation and per state change. That costs the radio on every interaction and
buys an operator nothing. A floor alone would cut that, and also cut the
context an error needs to be understood. With breadcrumbs, the context survives
the cut, and volume scales with faults rather than with use.
