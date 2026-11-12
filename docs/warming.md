# Warming

How the warmer keeps a prefix warm: when a pass runs, which targets it pings and in what order, and
what each answer leaves behind. The routes and fields named here are `docs/api.md`'s.

A provider keeps a prompt prefix cached for a while after its last read or write and then drops it.
A target that will be used again soon is cheaper read from a warm cache than rebuilt, so the warmer
asks its consumer to touch the prefix once it has stood idle long enough, and before the provider
drops it. A warm target stays warm: nothing but its own lifetime running out, its switch, its pause
or the round ends its warming.

## The pass

- **When.** The first pass runs the policy's start delay (30 seconds) after the service starts: a
  starting host has real work to do first and warming is upkeep. Then one pass runs every interval of
  the round: the one set over `PATCH /round`, or the policy's tick (60 seconds) while none is. A new
  interval moves the next pass to the last pass's time plus the new interval, or to now where that has
  passed.
- **Passes come on the beat.** Each pass starts when the beat says it is due, whether or not an
  earlier pass has ended, so passes may stand side by side. A pass asked for over `POST /passes` runs beside any other.
- **A failed pass is not the end.** A pass the store fails is written to standard error, and the next
  one comes as usual.
- **The round first.** While the round is not running, a pass judges nothing and pings nothing.
- **Each consumer has one line.** A consumer's pings go one at a time on its line, each waiting for
  its answer. The lines of different consumers run side by side, so a consumer slow to answer holds
  back its own targets and no one else's.
- **A line still out is passed over.** A pass first claims the line of every consumer whose line is
  free, then reads the targets and judges them, so what it judges was read after every ping those lines
  had out was recorded. A consumer whose line an earlier pass still holds is left to that pass: its
  targets are judged again by the first pass that finds its line free.
- **One target at a time, in rank order.** On each line the pass walks the consumer's due targets by
  the rank each registration declares, lowest first, then by target name; the order is stable from
  pass to pass.
- **Checked again before each ping.** Just before it posts a ping, the line reads the round, the
  target's subject switch and its pause again, so a switch thrown during a long pass holds from the
  next ping on. A round found stopped, or the service found stopping, ends the line there. A target
  found off, paused or retired is passed over, and no other target of its group takes its place in
  that pass.
- **No clock on a ping.** A ping waits for the hook's answer however long the consumer takes. It ends
  only in an answer, a connection refused, a request that breaks off, a hook host that has gone,
  which the connection's keep-alive probes tell, or the service's stop cutting its drain. A hook
  that never answers holds its consumer's line, and that line alone, until it does.
- **The stop.** Once the service is stopping, the ticker starts no pass, a pass asked for pings
  nothing, and a running pass waits only for the pings already out before it records its end. A ping
  the stop cuts, at its drain deadline or on a second signal, is not recorded, so its target keeps its record and is judged again by the
  first pass after the service is back (`docs/api.md`, 「The service」).
- **Recorded.** Each attempt ends with a `target.changed` event for its target, and each pass, once
  every line it claimed has come back, with a `pass.created` event.

## When a target is due

- **Judged by the schedule crate.** The rule below and the order of 「Groups」 are the crate
  `bmo-cache-warmer-schedule` (`docs/api.md`, 「The schedule crate」): a library with no dependency of
  its own, judging only from what it is handed, `now` included, which another repository may take
  alone. The service re-exports it as its `schedule` module. The warmer hands it each
  target's clocks, record, switch and pause, and the pass's moment, idle override and round; the
  clock, the store, the policy, the ping and what its answer leaves behind stay the service's.

A target's **touch** is the later of its last use and its last warmed ping, and, where it registered
`failureTouches`, its last failed attempt. Its **idle** is now less its touch. Its **idle stretch**
is its `idleMs`, or the pass's idle override where the pass has one and the target honours it.

A target is due in a pass when all of these hold:

1. **It is on.** Its subject's switch is on, or is unset and its registration warms by default. The
   policy's default for a registration that says nothing is off.
2. **It is not paused.** Its subject has no pause standing.
3. **The round is running.**
4. **It is idle enough.** It has no touch at all, or its idle is at least its idle stretch. A target
   never used and never pinged is due at once: this is how a session not yet standing is brought up.
5. **Its prefix is still there.** Its idle is below its lifetime, `ttlMs`, less its margin. A lifetime
   no larger than its margin leaves nothing, and the target is never due.

A prefix already dropped is left alone. Pinging it would rebuild it at the cost of the work it is
waiting for, and if that work never comes the ping is all that was spent.

The library also judges a lifetime that is not declared at all as no limit, for a caller whose
targets have none; the service always has one, since a registration declares `ttlMs`.

## Groups

- **Closest to cooling first.** Of a group's due targets, the pass pings those with the oldest touch
  first, up to the group's limit: a target never touched counts oldest of all, and targets of equal
  touch keep their rank order. A group with a limit of 0 has nothing pinged.
- **Where the group stands.** A group's targets go at the rank of the group's first target in rank
  order, together.
- **One group each.** A target belongs to at most one group, so no prefix is pinged twice in a pass.

## What an answer leaves behind

- **The last attempt** — its time, outcome, code and reason — is recorded for every ping, whatever it
  answered, and goes into the attempt log (`docs/store.md`). An attempt's time is the moment its
  answer arrived, or the moment the hook could not be reached or its request broke off.
- **The last failed attempt's time** is kept apart from the last attempt, so a skipped or busy answer
  after a failure does not lose the failure's touch.
- **Only a warmed answer restarts the clock.** Its arrival becomes the target's last warmed time, and
  so its touch.
- **A busy answer** means work was running on the target, keeping the prefix warm itself. It moves
  nothing else; the use the work records when it ends moves the touch.
- **A skipped answer** moves nothing else; the target is judged again next pass.
- **A failed answer** moves nothing else, unless the target registered `failureTouches`. Then the
  attempt counts as a touch: the target is not asked again until its idle stretch has passed since,
  and its lifetime is measured from it. A target that did not register it is asked again on the next
  pass.
- **The reason** a failed answer gives, or the warmer's own when the hook failed, is cut to the
  policy's length of UTF-16 units, the way a script counts a string; a surrogate pair the cut splits
  ends as the replacement character.

## The switches

- **The round** stops every warming at once while it is not running, and holds until set running.
- **A subject's switch** turns all of one subject's targets on or off and holds until thrown back;
  unset, each target follows its registration's default. A subject can be switched before any target
  names it.
- **A pause** turns a subject off from the moment it is set until its `pausedUntil`, or until a use
  whose work began after the pause was set, whichever comes first. Work already running when the pause
  was set does not lift it. It never changes the switch, which still wins while it is off.
- **Records stand still.** A target passed over because it is off, paused or stopped keeps its record
  where it was, and is judged from that record once it is on again.

## Uses

- A consumer records each use of a target with when its prefix was last written and when the work
  began. The touch moves to the write where that is later; a use older than the record moves nothing.
- A registration may carry the last use the consumer already knows, so a target registered after its
  session has stood a while is judged from that session's last write rather than brought up at once.

## The policy

The warmer's own fixed numbers stand in `policy/warmer.toml`, built into the binary and read once as
the service starts. A key the reader does not know, a missing key, or a value of 0 where one must be
above it refuses the file, naming what is wrong, so a misspelling never reads as a default.

| Key | Value | Meaning |
| --- | --- | --- |
| `start_delay_seconds` | 30 | how long after the service starts the first pass waits |
| `tick_seconds` | 60 | the round's interval while none is set, above 0 |
| `warm_by_default` | false | whether a target warms when neither its registration nor its subject's switch says |
| `idle_percent` | 66 | a registration's idle stretch where it states none: this share of its lifetime less its margin, at least one millisecond, 1 to 100 |
| `reason_units` | 400 | the UTF-16 units a failed attempt's reason is cut to, above 0 |
| `log_days` | 14 | how many days an attempt stays in the attempt log, above 0 |
| `event_days` | 2 | how many days an event stays in the event window, above 0 |
| `drain_seconds` | 15 | how long a stop waits for the calls, pings and passes in flight before it cuts them, above 0 |

A target's clocks are its registration's; the policy fills in only what a registration leaves out.
No policy value bounds how long a ping, a line or a pass may take while the service runs; only a
stop's drain deadline cuts them.

## What the warmer leaves to the consumer

Whether a provider is on, whether an account can answer, whether work is running, which line a ping
goes into and what it says are the consumer's, answered through the hook. Which targets exist at all
is the consumer's, stated by what it registers and retires (`docs/api.md`, 「What stays the
consumer's」).
