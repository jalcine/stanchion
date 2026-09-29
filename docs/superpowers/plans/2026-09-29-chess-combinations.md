# Chess combinations expansion — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 50 loaded combination plugins (40 new) on a defender-graph XRPC contract.

**Architecture:** `board.gd` gains one attacker enumerator + 4 per-move fields +
2 position maps; 40 folders under `examples/chess/plugins/` following the
existing `init.lua` + `plugin.toml` pattern; `main.gd` DIFFICULTY extended.

**Tech Stack:** GDScript (Godot 4.7), Lua (sandboxed via Stanchion), mise.

---

### Task 1: Engine attacker enumerator

**Files:**
- Modify: `examples/chess/src/board.gd:83-128` (attack detection section)

- [ ] **Step 1: Add `_attackers_of`, rebase `is_attacked` on it**

```gdscript
## Squares holding `by_color` pieces that attack `sq`. Single source behind
## is_attacked and the defender-graph annotations.
func _attackers_of(sq: int, by_color: String) -> Array:
	var out: Array = []
	var tf := file_of(sq)
	var tr := rank_of(sq)

	var pawn_rank := tr - 1 if by_color == "w" else tr + 1
	for df in [-1, 1]:
		if in_board(tf + df, pawn_rank):
			var psq := pawn_rank * 8 + tf + df
			if squares[psq] == by_color + "P":
				out.append(psq)

	const KNIGHT := [[1, 2], [2, 1], [2, -1], [1, -2], [-1, -2], [-2, -1], [-2, 1], [-1, 2]]
	for step in KNIGHT:
		if in_board(tf + step[0], tr + step[1]):
			var nsq := (tr + step[1]) * 8 + tf + step[0]
			if squares[nsq] == by_color + "N":
				out.append(nsq)

	const KING := [[1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0], [-1, -1], [0, -1], [1, -1]]
	for step in KING:
		if in_board(tf + step[0], tr + step[1]):
			var ksq := (tr + step[1]) * 8 + tf + step[0]
			if squares[ksq] == by_color + "K":
				out.append(ksq)

	const ROOK_DIRS := [[1, 0], [-1, 0], [0, 1], [0, -1]]
	_collect_ray_attackers(tf, tr, ROOK_DIRS, by_color, ["R", "Q"], out)
	const BISHOP_DIRS := [[1, 1], [1, -1], [-1, 1], [-1, -1]]
	_collect_ray_attackers(tf, tr, BISHOP_DIRS, by_color, ["B", "Q"], out)
	return out

func _collect_ray_attackers(tf: int, tr: int, dirs: Array, by_color: String, types: Array, out: Array) -> void:
	for dir in dirs:
		var f: int = tf + int(dir[0])
		var r: int = tr + int(dir[1])
		while in_board(f, r):
			var code := squares[r * 8 + f]
			if code != "":
				if color_of(code) == by_color and types.has(type_of(code)):
					out.append(r * 8 + f)
				break
			f += int(dir[0])
			r += int(dir[1])
```

Replace `is_attacked` body with `return not _attackers_of(sq, by_color).is_empty()`
(doc comment stays). Keep `_ray_hits` only if still referenced — otherwise delete it.

- [ ] **Step 2: Run the game, expect zero regressions**

Run: `timeout 3s mise chess 2>&1`
Expected: no SCRIPT ERRORs (behavior identical; pure refactor).

### Task 2: Annotated-move + position fields

**Files:**
- Modify: `examples/chess/src/board.gd` (`annotated_moves`, `position_for`)

- [ ] **Step 1: Add fields in `annotated_moves`** (after `to_is_attacked` is computed):

```gdscript
		var mover_value: int = VALUE.get(type_of(squares[move["from"]]), 0)
		var captured_defended := not _attackers_of(to, foe).is_empty()
		var attackers_of_to := trial._attackers_of(landed, foe)
		var foe_king := trial.king_square(foe)
		var is_double := gives_check and foe_king != -1 and trial._attackers_of(foe_king, color).size() >= 2
```

Append to the `out.append({...})` dict:
```gdscript
			"mover_value": mover_value,
			"captured_defended": captured_defended,
			"attackers_of_to": attackers_of_to,
			"is_double_check": is_double,
```

(`trial`/`to`/`foe`/`landed`/`gives_check` already exist in that scope; `trial`
is untyped so method calls are dynamic — same pattern as the existing code.)

- [ ] **Step 2: Add attack maps in `position_for`**:

```gdscript
func _attack_map(by_color: String) -> Dictionary:
	var map := {}
	for sq in 64:
		var hit := _attackers_of(sq, by_color)
		if not hit.is_empty():
			map[sq] = hit
	return map
```

```gdscript
		"foe_attacks": _attack_map(opponent(color)),
		"own_attacks": _attack_map(color),
```

- [ ] **Step 3: Verify**

Run: `timeout 3s mise chess 2>&1`
Expected: 10 loaded, zero SCRIPT ERRORs.

- [ ] **Step 4: Commit**

```bash
git add examples/chess/src/board.gd
git commit -m "feat(chess): defender-graph annotations for plugins"
```

### Task 3–7: Plugin batches (8 per batch, 5 batches)

Pattern per plugin — `examples/chess/plugins/<folder>/plugin.toml`:
```toml
name = "<folder>"
version = "1.0.0"
```
`init.lua` skeleton (ClassName mirrors folder):
```lua
-- "<Display>": one-line trigger description + which annotations it reads.
local X = {}
X.__index = X

function X.new(config, deps)
  return setmetatable({}, X)
end

function X:name() return "Display" end

function X:suggest(position)
  -- scan position.moves, return nil or
  -- { from=, to=, promote=, strength=, name=, rationale= }
end

return X
```
Per-plugin triggers/strengths: see spec §The 40. Batch commits after each batch
of 8 (`feat(chess): 8 more combinations (batch N)`).

### Task 8: Difficulties + verify

- [ ] Extend `DIFFICULTY` in `examples/chess/src/main.gd:38-42` per spec.
- [ ] `timeout 60 mise exec -- godot --headless --import` in `examples/chess`,
      then `timeout 3s mise chess` → "Loaded combinations: …" lists 50, no errors.
- [ ] Commit. Done; phase 2 (restructure) is separate work.
