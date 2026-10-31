#!/usr/bin/env python3
"""Loop dashboard. Follows the newest .loop/sessions/*.jsonl (claude -p stream-json) plus
.loop/events.log and redraws in place. Strictly read-only, so quitting never affects the loop.

usage: loop/watch.sh [session.jsonl] [--once]
"""
import glob, json, os, shutil, signal, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STAGES = ["SCOUT", "BUILD", "CHECK", "AUDIT", "COMMIT"]
SPIN = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
COLOR = {"SCOUT": 36, "BUILD": 34, "CHECK": 33, "AUDIT": 35, "COMMIT": 32, "LOOP": 32, "SESSION": 37}
DIM, BOLD, RED, GREEN = "\033[2m", "\033[1m", "\033[31m", "\033[32m"
RESET = "\033[0m"


def paint(text, code):
    return f"\033[{code}m{text}{RESET}"


class Session:
    """State derived from one session's stream-json log."""

    def __init__(self, path):
        self.path, self.offset, self.buf = path, 0, ""
        self.spawns = {}        # Agent tool_use id -> stage label (SCOUT/BUILD/AUDIT)
        self.stage, self.stage_active = None, False
        self.ended = False
        self.feed = []          # (clock, tag, text)
        self.last_activity = os.path.getmtime(path) if path else time.time()

    def poll(self):
        if not self.path:
            return
        with open(self.path, "r", errors="replace") as f:
            f.seek(self.offset)
            chunk = f.read()
            self.offset = f.tell()
        if not chunk:
            return
        self.last_activity = time.time()
        self.buf += chunk
        *lines, self.buf = self.buf.split("\n")
        for line in lines:
            try:
                self.handle(json.loads(line))
            except (ValueError, AttributeError, KeyError, TypeError):
                pass

    def say(self, tag, text):
        self.feed.append((time.strftime("%H:%M:%S"), tag, " ".join(str(text).split())))
        del self.feed[:-200]

    def enter(self, stage):
        self.stage, self.stage_active = stage, True

    def handle(self, ev):
        kind, parent = ev.get("type"), ev.get("parent_tool_use_id")
        tag = self.spawns.get(parent, "SESSION")
        if kind == "result":
            self.ended = True
            self.enter("COMMIT")
            self.say("SESSION", f"session ended ({ev.get('subtype')}); loop is gating")
            return
        for block in (ev.get("message") or {}).get("content") or []:
            if not isinstance(block, dict):
                continue
            if kind == "assistant" and block.get("type") == "tool_use":
                self.tool_use(tag, parent, block)
            elif kind == "assistant" and block.get("type") == "text" and parent is None:
                self.say(tag, block.get("text", "").strip().split("\n")[0])
            elif kind == "user" and block.get("type") == "tool_result" and parent is None:
                done = self.spawns.get(block.get("tool_use_id"))
                if done:
                    self.say(done, "finished")
                    if done == self.stage:
                        self.stage_active = False

    def tool_use(self, tag, parent, block):
        name, args = block.get("name"), block.get("input") or {}
        if name == "Agent" and parent is None:
            label = args.get("description", "agent")
            stage = label.split(":")[0].strip().upper()
            self.spawns[block.get("id")] = stage if stage in STAGES else "SESSION"
            self.enter(self.spawns[block.get("id")])
            self.say(self.spawns[block.get("id")], f"spawned '{label}' on {args.get('model', 'default')}")
            return
        detail = args.get("command") or args.get("file_path") or args.get("pattern") or args.get("path") or ""
        if parent is None and name == "Bash" and "judges/" in detail:
            self.enter("CHECK")
            tag = "CHECK"
        self.say(tag, f"{name} {detail}")


def tasks():
    try:
        lines = open(os.path.join(ROOT, "TASKS.md")).read().splitlines()
    except OSError:
        return {}
    out = {}
    for line in lines:
        if line.startswith("- [") and ": " in line:
            ident, title = line[6:].split(": ", 1)
            out[ident] = (line[3], title)
    return out


def loop_events():
    try:
        lines = open(os.path.join(ROOT, ".loop/events.log")).read().splitlines()[-200:]
    except OSError:
        return []
    return [(l[:8], "LOOP", l[14:]) for l in lines if len(l) > 14]


def newest_log():
    logs = glob.glob(os.path.join(ROOT, ".loop/sessions/*.jsonl"))
    return max(logs, key=os.path.getmtime) if logs else None


def render(s, tick, width, height):
    ts = tasks()
    total = len(ts)
    done = sum(1 for m, _ in ts.values() if m == "x")
    blocked = sum(1 for m, _ in ts.values() if m == "!")
    bar_w = max(10, min(40, width - 40))
    n_done = bar_w * done // total if total else 0
    n_block = bar_w * blocked // total if total else 0
    bar = GREEN + "█" * n_done + RED + "█" * n_block + RESET + DIM + "░" * (bar_w - n_done - n_block) + RESET
    rows = [f"{BOLD}openbim loop{RESET}  {bar}  {done}/{total} done  " + paint(f"{blocked} blocked", 31 if blocked else 2)]

    name = os.path.basename(s.path) if s.path else ""
    parts = name.replace(".jsonl", "").split("-")
    task_id, attempt = (parts[2], parts[3].lstrip("a")) if len(parts) >= 4 else ("—", "—")
    title = ts.get(task_id, ("", ""))[1]
    rows.append(f"task {BOLD}{task_id}{RESET} {title[:width - 30]}  attempt {attempt}")

    events = loop_events()
    outcome = next((e for e in reversed(events) if task_id in e[2] and e[2].split()[0] in
                    ("COMMITTED", "FAILED", "BLOCKED")), None) if s.ended else None
    cells = []
    cur = STAGES.index(s.stage) if s.stage in STAGES else -1
    for i, st in enumerate(STAGES):
        if i < cur or (i == cur and not s.stage_active):
            cell = paint(f"✓ {st}", COLOR[st])
        elif i == cur and st == "COMMIT" and outcome:
            ok = outcome[2].startswith("COMMITTED")
            cell = paint(f"{'✓' if ok else '✗'} {st}", 32 if ok else 31)
        elif i == cur:
            cell = BOLD + paint(f"{SPIN[tick % len(SPIN)]} {st}", COLOR[st])
        else:
            cell = DIM + f"· {st}" + RESET
        cells.append(cell)
    rows.append("  " + "  →  ".join(cells))
    idle = int(time.time() - s.last_activity)
    rows.append(DIM + f"last activity {idle}s ago   log {name or '(none yet)'}" + RESET)
    rows.append("")

    feed = sorted(s.feed + [e for e in events if e[0] >= (s.feed[0][0] if s.feed else "")],
                  key=lambda e: e[0])
    room = max(3, height - len(rows) - 1)
    for clock, tag, text in feed[-room:]:
        rows.append(f"{DIM}{clock}{RESET} " + paint(f"{tag:<7}", COLOR.get(tag, 37)) + " " + text[:max(10, width - 18)])
    return rows


def quit_on_term(*_):
    raise KeyboardInterrupt


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    once = "--once" in sys.argv
    signal.signal(signal.SIGTERM, quit_on_term)
    fixed = os.path.abspath(args[0]) if args else None
    session = Session(fixed or newest_log())
    tick = 0
    if not once:
        sys.stdout.write("\033[?25l\033[2J")      # hide cursor, clear once at start
    try:
        while True:
            path = fixed or newest_log()
            if path != session.path:
                session = Session(path)
            session.poll()
            width, height = shutil.get_terminal_size((100, 30))
            rows = render(session, tick, width, height)
            if once:
                print("\n".join(rows))
                return
            sys.stdout.write("\033[H" + "".join(r + "\033[K\n" for r in rows) + "\033[J")
            sys.stdout.flush()
            tick += 1
            time.sleep(0.25)
    except KeyboardInterrupt:
        pass
    finally:
        if not once:
            sys.stdout.write("\033[?25h\n")


if __name__ == "__main__":
    main()
