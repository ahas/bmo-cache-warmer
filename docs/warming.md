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
  starting host has real work to do first and warming is upkeep. Then one pass runs every tick (60
  seconds).
- **Never beside itself.** One pass runs at a time. A tick that comes while a pass is still running is
  put off until it ends, and a tick missed that way is delayed rather than run twice to catch up. A
  pass asked for over `POST /pass` while one runs is refused.
- **The round first.** While the owner has stopped the round, a pass judges nothing and pings nothing.
- **One target at a time, in rank order.** The pass walks targets by the rank each registration
  declares, lowest first, then by consumer and target name; the order is stable from pass to pass.
  It waits for each ping's answer before the next.
- **Checked again before each ping.** Just before it posts a ping, the pass reads the round, the
  target's subject switch and its pause again, so a switch thrown during a long pass holds from the
  next ping on.
- **Bounded.** A ping is given the target's ping bound plus the policy's slack. A hook that has not
  answered by then is recorded failed, and the pass goes on.

## When a target is due

A target's **touch** is the later of its last use and its last warmed ping, and, where it registered
`failure_touches`, its last failed attempt. Its **idle** is now less its touch. Its **idle stretch**
is its `idle_ms`, or the pass's idle override where the pass has one and the target honours it.

A target is due in a pass when all of these hold:

1. **It is on.** Its subject's switch is on, or is unset and its registration warms by default. The
   policy's default for a registration that says nothing is off.
2. **It is not paused.** Its subject has no pause standing.
3. **The round is not stopped.**
4. **It is idle enough.** It has no touch at all, or its idle is at least its idle stretch. A target
   never used and never pinged is due at once: this is how a session not yet standing is brought up.
5. **Its prefix is still there.** Where it declares a lifetime, its idle is below the lifetime less
   its margin. A lifetime no larger than its margin leaves nothing, and the target is never due. A
   target that declares no lifetime has no such limit: the consumer resumes it as it stands.

A prefix already dropped is left alone. Pinging it would rebuild it at the cost of the work it is
waiting for, and if that work never comes the ping is all that was spent.

## Groups

- **Closest to cooling first.** Of a group's due targets, the pass pings those with the oldest touch
  first, up to the group's limit. A group with a limit of 0 has nothing pinged.
- **Where the group stands.** A group's targets go at the rank of the group's first target in rank
  order, together.
- **One group each.** A target belongs to at most one group, so no prefix is pinged twice in a pass.

## What an answer leaves behind

- **The last attempt** — its time, outcome, code and reason — is recorded for every ping, whatever it
  answered.
- **Only a warmed answer restarts the clock.** Its arrival becomes the target's last warmed time, and
  so its touch.
- **A busy answer** means work was running on the target, keeping the prefix warm itself. It moves
  nothing else; the use report the work makes when it ends moves the touch.
- **A skipped answer** moves nothing else; the target is judged again next pass.
- **A failed answer** moves nothing else, unless the target registered `failure_touches`. Then the
  attempt counts as a touch: the target is not asked again until its idle stretch has passed since,
  and its lifetime is measured from it. Launch lines, editors, gates and worker sessions hold their
  records that way; pool places are asked again on the next pass.
- **The reason** a failed answer gives, or the warmer's own when the hook failed, is cut to the
  policy's length of UTF-16 units, the way a script counts a string; a surrogate pair the cut splits
  ends as the replacement character.

## The owner's switches

- **The round** stops every warming at once and holds until thrown back.
- **A subject's switch** turns all of one subject's targets on or off and holds until thrown back;
  unset, each target follows its registration's default. A subject can be switched before any target
  names it.
- **A pause** turns a subject off until its next use. A use report whose work began after the pause
  clears it; work already running when the pause was set does not. Nothing else clears it, and it
  never changes the switch, which still wins while it is off.
- **Records stand still.** A target passed over because it is off, paused or stopped keeps its record
  where it was, and is judged from that record once it is on again.

## Use reports

- A consumer reports each use of a target with when its prefix was last written and when the work
  began. The touch moves to the write where that is later; a report older than the record moves
  nothing.
- A registration may carry the last use the consumer already knows, so a target registered after its
  session has stood a while is judged from that session's last write rather than brought up at once.

## The policy

The warmer's own fixed numbers stand in `policy/warmer.toml`, built into the binary and read once as
the service starts. A key the reader does not know, a missing key, or a clock of 0 refuses the file,
naming what is wrong, so a misspelling never reads as a default.

| Key | Value | Meaning |
| --- | --- | --- |
| `start_delay_seconds` | 30 | how long after the service starts the first pass waits |
| `tick_seconds` | 60 | how often a pass comes round, above 0 |
| `warm_by_default` | false | whether a target warms when neither its registration nor its subject's switch says |
| `ping_slack_seconds` | 15 | added to a target's ping bound before the warmer gives up on the hook, for the hook's own transport and answer |
| `reason_units` | 400 | the UTF-16 units a failed attempt's reason is cut to, above 0 |

Every clock a target runs on — idle stretch, lifetime, margin, ping bound — is its registration's,
never this file's.

## What the warmer leaves to the consumer

Whether a provider is on, whether an account can answer, whether a turn is running, which line a ping
goes into and what it says are the consumer's, answered through the hook. Which targets exist at all —
which workers, which support workers, which pool places — is the consumer's, stated by what it
registers and retires. The table in `docs/api.md` (「Consumers and their targets」) states each
consumer's clocks.
