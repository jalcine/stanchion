## Chess, where the *rules* live here in Godot and the *plays* live in Lua.
##
## Every piece's movement, check and checkmate is `ChessBoard` (src/board.gd). What is
## loaded through Stanchion is a folder of "combination" plugins — mate-in-one, win
## material, knight fork, castle, control the centre, develop — each a sandboxed Lua
## script that reads the annotated position and proposes a move.
##
## The same catalogue does double duty: the AI picks the strongest applicable
## combination (and which combinations it is *allowed* to see sets the difficulty),
## while the player is shown every combination available to them as a hint.
extends Control

# The board art is 142×142 with a 7px frame, so the playable 8×8 grid is 16px cells
# inset by 7px (7 + 8*16 + 7 = 142). We render the whole texture at BOARD_PX and derive
# the cell size and inset from those same ratios, so pieces land on the drawn squares
# instead of drifting.
const BOARD_PX := 640.0
const TEX_SIZE := 142.0
const TEX_FRAME := 7.0
const TEX_CELL := 16.0
const SCALE := BOARD_PX / TEX_SIZE
const ORIGIN := TEX_FRAME * SCALE
const CELL := TEX_CELL * SCALE

# Draw the expected cell bounding boxes over the board (a debug aid for alignment).
const SHOW_CELL_BORDERS := false
const GLYPH := {"K": "♚", "Q": "♛", "R": "♜", "B": "♝", "N": "♞", "P": "♟"}

# Per-colour glyph nudges, as a fraction of a cell (x: +right, y: +down). White reads a
# touch left and up; black a touch left and down.
const WHITE_NUDGE := Vector2(0.0, 0.0)
const BLACK_NUDGE := Vector2(0.0, 0.0)

# Which combinations the AI is allowed to consult at each difficulty. Fewer, softer
# combinations make a gentler opponent; the tactical ones make it bite.
const DIFFICULTY := {
	"Gentle": ["center_control", "develop_pieces"],
	"Steady": ["center_control", "develop_pieces", "castle_safety", "win_material"],
	"Sharp": ["center_control", "develop_pieces", "castle_safety", "win_material", "knight_fork", "checkmate"],
}

var stanchion  # the GDExtension Stanchion class, or null if the extension is missing
var board := ChessBoard.new()
var human := "w"
var difficulty := "Steady"

var selected := -1
var selected_moves: Array = []  # legal moves from the selected square
var hint_move: Dictionary = {}  # the top combination suggestion for the human
var ai_last_move: Dictionary = {}  # from/to of Black's last move, tinted while shown
var game_over := ""

var pieces_layer: Node2D
var board_font: Font
var overlay: Node2D
var status_label: Label
var hint_label: RichTextLabel
var ai_label: RichTextLabel
var log_label: RichTextLabel
var move_log: Array[String] = []


func _ready() -> void:
	_build_ui()
	_open_stanchion()
	_refresh()


# ---- Stanchion wiring -------------------------------------------------------

func _open_stanchion() -> void:
	if not ClassDB.class_exists("Stanchion"):
		_note("Stanchion extension not built — see the README. Running rules only, no AI plays.")
		return
	stanchion = ClassDB.instantiate("Stanchion")
	var plugins_path := ProjectSettings.globalize_path("res://plugins")
	# No capabilities, tight-ish limits: these plugins only read a table and return one.
	var ok: bool = stanchion.open({"plugins": plugins_path})
	if not ok:
		_note("Stanchion.open failed: " + str(stanchion.get_last_error()))
		stanchion = null
		return
	var report: Dictionary = stanchion.load("")
	var loaded: Array = report.get("loaded", [])
	if loaded.is_empty():
		_note("No combination plugins loaded from " + plugins_path)
	else:
		_note("Loaded combinations: " + ", ".join(loaded))


## Asks every loaded combination for its move, returning `{ plugin: suggestion }` for
## the ones that apply. `allowed` filters which combinations may answer (difficulty).
func gather_suggestions(color: String, allowed: Array) -> Array:
	if stanchion == null:
		return []
	var outcomes: Array = stanchion.dispatch("suggest", [board.position_for(color)])
	var picks: Array = []
	for outcome in outcomes:
		var name: String = outcome.get("plugin", "")
		if not allowed.has(name):
			continue
		var value = outcome.get("value")
		if value is Dictionary and value.has("from"):
			value["plugin"] = name
			picks.append(value)
	picks.sort_custom(func(a, b): return int(a.get("strength", 0)) > int(b.get("strength", 0)))
	return picks


# ---- turn flow --------------------------------------------------------------

func _human_turn() -> bool:
	return board.side_to_move == human and game_over == ""


func _try_human_move(from: int, to: int) -> void:
	for move in board.legal_moves(human):
		if move["from"] == from and move["to"] == to:
			# Auto-queen: prefer the promotion-to-queen variant when one exists.
			if move.get("promote", "") != "" and move["promote"] != "Q":
				continue
			_commit(move)
			return


func _commit(move: Dictionary) -> void:
	move_log.append(_describe(move, board.side_to_move))
	board.apply(move)
	selected = -1
	selected_moves = []
	hint_move = {}
	_check_end()
	_refresh()
	if game_over == "" and not _human_turn():
		# Let the board paint before the AI thinks.
		await get_tree().create_timer(0.35).timeout
		_ai_move()


func _ai_move() -> void:
	var color := board.side_to_move
	var picks := gather_suggestions(color, DIFFICULTY[difficulty])
	var chosen := _match_suggestion(picks, color)
	if chosen.is_empty():
		# No combination applied: fall back to a plain "grab what you can, else a
		# random legal move" so the game always continues.
		chosen = _fallback_move(color)
		_show_ai_reasoning({}, picks, chosen)
	else:
		_show_ai_reasoning(picks[0], picks, chosen)
	if chosen.is_empty():
		_check_end()
		_refresh()
		return
	# Remember the squares Black just reasoned about, so they stay tinted on the board
	# while the player reads the reasoning.
	ai_last_move = {"from": chosen["from"], "to": chosen["to"]}
	_commit(chosen)


## Writes what Black just decided into the side panel: the combination it played, why,
## and which other combinations it weighed (so the reasoning is legible, not a black box).
func _show_ai_reasoning(chosen_pick: Dictionary, picks: Array, chosen: Dictionary) -> void:
	if ai_label == null:
		return
	if chosen.is_empty():
		ai_label.text = "[i]No legal move.[/i]"
		return
	var move_text := _describe(chosen, board.side_to_move)
	var text := ""
	if chosen_pick.is_empty():
		text = "[b]%s[/b]\n[color=#c9a][i]No combination applied — improvised.[/i][/color]\n" % move_text
	else:
		text = "[b]%s — %s[/b]\n%s\n" % [move_text, chosen_pick.get("name", ""), chosen_pick.get("rationale", "")]
	# Show the combinations it considered but passed over, at this difficulty.
	if picks.size() > 1:
		text += "\n[color=#888]Also weighed:[/color]\n"
		for i in range(1, mini(picks.size(), 4)):
			text += "[color=#888]· %s (%d)[/color]\n" % [picks[i].get("name", ""), int(picks[i].get("strength", 0))]
	_note("AI played: " + move_text)
	ai_label.text = text


## Turns a plugin's `{from,to,promote}` back into the engine's own legal move (with the
## correct castle / en-passant flags), rejecting anything not currently legal.
func _match_suggestion(picks: Array, color: String) -> Dictionary:
	var legal := board.legal_moves(color)
	for pick in picks:
		for move in legal:
			if move["from"] == pick["from"] and move["to"] == pick["to"]:
				var want := str(pick.get("promote", ""))
				if want == "" or move.get("promote", "") == want:
					return move
	return {}


func _fallback_move(color: String) -> Dictionary:
	var best := {}
	var best_gain := -1
	for info in board.annotated_moves(color):
		if info["captured_value"] > best_gain:
			best_gain = info["captured_value"]
			best = info["raw"]
	if best.is_empty():
		return {}
	if best_gain <= 0:
		# Nothing to take — pick any legal move so play stays lively.
		var legal := board.legal_moves(color)
		if not legal.is_empty():
			return legal[randi() % legal.size()]
	return best


func _check_end() -> void:
	var color := board.side_to_move
	if board.legal_moves(color).is_empty():
		if board.in_check(color):
			var winner := "White" if color == "b" else "Black"
			game_over = winner + " wins by checkmate"
		else:
			game_over = "Draw by stalemate"


# ---- input ------------------------------------------------------------------

func _on_board_input(event: InputEvent) -> void:
	if not (event is InputEventMouseButton and event.pressed and event.button_index == MOUSE_BUTTON_LEFT):
		return
	if not _human_turn():
		return
	var sq := _square_at(event.position)
	if sq == -1:
		return
	if selected == -1:
		if board.color_of(board.squares[sq]) == human:
			_select(sq)
	elif sq == selected:
		_select(-1)
	else:
		var found := false
		for move in selected_moves:
			if move["to"] == sq:
				found = true
				break
		if found:
			_try_human_move(selected, sq)
		elif board.color_of(board.squares[sq]) == human:
			_select(sq)
		else:
			_select(-1)


func _select(sq: int) -> void:
	selected = sq
	selected_moves = []
	if sq != -1:
		for move in board.legal_moves(human):
			if move["from"] == sq:
				selected_moves.append(move)
	overlay.queue_redraw()


func _square_at(pos: Vector2) -> int:
	var gx := pos.x - ORIGIN
	var gy := pos.y - ORIGIN
	if gx < 0 or gy < 0 or gx >= CELL * 8 or gy >= CELL * 8:
		return -1
	var file := int(gx / CELL)
	var rank := 7 - int(gy / CELL)
	return rank * 8 + file


func _cell_pos(sq: int) -> Vector2:
	return Vector2(ORIGIN + ChessBoard.file_of(sq) * CELL, ORIGIN + (7 - ChessBoard.rank_of(sq)) * CELL)


# ---- rendering --------------------------------------------------------------

func _refresh() -> void:
	pieces_layer.queue_redraw()

	# Refresh the human's available combinations as a hint.
	hint_move = {}
	var hint_text := ""
	if _human_turn():
		var picks := gather_suggestions(human, DIFFICULTY["Sharp"])  # show them everything
		if not picks.is_empty():
			hint_move = picks[0]
			for pick in picks:
				hint_text += "[b]%s[/b]  %s\n" % [pick.get("name", ""), pick.get("rationale", "")]
		else:
			hint_text = "[i]No named combination here — trust a principle.[/i]"
	hint_label.text = hint_text

	if game_over != "":
		status_label.text = game_over
	elif _human_turn():
		status_label.text = "Your move (White)"
	else:
		status_label.text = "Black is thinking…"

	log_label.text = ""
	var n := move_log.size()
	var start := maxi(0, n - 16)
	for i in range(start, n):
		log_label.text += "%d. %s\n" % [i + 1, move_log[i]]

	overlay.queue_redraw()


func _draw_overlay() -> void:
	# Squares the current reasoning refers to, tinted behind the pieces: amber for the
	# move Black just played, blue for the player's top suggested combination.
	if ai_last_move.has("from"):
		_tint(ai_last_move["from"], Color(0.95, 0.6, 0.2, 0.32))
		_tint(ai_last_move["to"], Color(0.95, 0.6, 0.2, 0.32))
	if _human_turn() and hint_move.has("from"):
		_tint(hint_move["from"], Color(0.3, 0.6, 1.0, 0.28))
		_tint(hint_move["to"], Color(0.3, 0.6, 1.0, 0.28))

	# Selected square and its legal destinations.
	if selected != -1:
		overlay.draw_rect(Rect2(_cell_pos(selected), Vector2(CELL, CELL)), Color(1, 0.9, 0.3, 0.35))
	for move in selected_moves:
		var center: Vector2 = _cell_pos(move["to"]) + Vector2(CELL, CELL) / 2
		var is_capture := board.squares[move["to"]] != ""
		if is_capture:
			overlay.draw_arc(center, CELL * 0.42, 0, TAU, 32, Color(0.9, 0.3, 0.2, 0.8), 4.0)
		else:
			overlay.draw_circle(center, CELL * 0.16, Color(0.2, 0.7, 0.3, 0.6))
	# The top suggested combination, drawn as an arrow-ish pair of markers.
	if _human_turn() and hint_move.has("from"):
		var a: Vector2 = _cell_pos(hint_move["from"]) + Vector2(CELL, CELL) / 2
		var b: Vector2 = _cell_pos(hint_move["to"]) + Vector2(CELL, CELL) / 2
		overlay.draw_line(a, b, Color(0.3, 0.6, 1.0, 0.7), 5.0)
		overlay.draw_circle(b, CELL * 0.12, Color(0.3, 0.6, 1.0, 0.9))


## Draws the pieces as font glyphs, each measured and centred in its square. White
## reads light with a dark outline; black the reverse.
## Fills one square with a translucent colour.
func _tint(sq: int, color: Color) -> void:
	overlay.draw_rect(Rect2(_cell_pos(sq), Vector2(CELL, CELL)), color)


func _draw_pieces() -> void:
	if SHOW_CELL_BORDERS:
		for sq in 64:
			pieces_layer.draw_rect(Rect2(_cell_pos(sq), Vector2(CELL, CELL)), Color(1, 0, 0, 0.7), false, 1.0)

	var font_size := int(CELL * 0.82)
	var outline := int(CELL * 0.06)
	for sq in 64:
		var code := board.squares[sq]
		if code == "":
			continue
		var glyph: String = GLYPH[board.type_of(code)]
		var is_white := board.color_of(code) == "w"
		var fill := Color(0.98, 0.98, 0.95) if is_white else Color(0.11, 0.11, 0.13)
		var edge := Color(0.1, 0.1, 0.1) if is_white else Color(0.9, 0.9, 0.9)

		# Centre the glyph on the square using its measured size and the font baseline,
		# then nudge per colour — the black and white glyph ink sits slightly
		# differently in the em box, so each needs its own small correction.
		var extent := board_font.get_string_size(glyph, HORIZONTAL_ALIGNMENT_LEFT, -1, font_size)
		var center: Vector2 = _cell_pos(sq) + Vector2(CELL, CELL) / 2
		var nudge := WHITE_NUDGE if is_white else BLACK_NUDGE
		var origin := Vector2(
			center.x - extent.x / 2.0 + nudge.x * CELL,
			center.y - extent.y / 2.0 + board_font.get_ascent(font_size) + nudge.y * CELL
		)
		pieces_layer.draw_string_outline(board_font, origin, glyph, HORIZONTAL_ALIGNMENT_LEFT, -1, font_size, outline, edge)
		pieces_layer.draw_string(board_font, origin, glyph, HORIZONTAL_ALIGNMENT_LEFT, -1, font_size, fill)


func _note(text: String) -> void:
	print("[chess] ", text)
	if status_label != null:
		status_label.tooltip_text = text


func _describe(move: Dictionary, color: String) -> String:
	var piece := board.type_of(board.squares[move["from"]])
	var prefix := "" if piece == "P" else piece
	var take := "x" if board.squares[move["to"]] != "" or move.get("en_passant", false) else "-"
	if move.get("castle", "") == "k":
		return "O-O"
	if move.get("castle", "") == "q":
		return "O-O-O"
	var promo: String = ""
	if move.get("promote", "") != "":
		promo = "=" + str(move["promote"])
	return "%s%s%s%s%s" % [prefix, ChessBoard.square_name(move["from"]), take, ChessBoard.square_name(move["to"]), promo]


# ---- UI construction (kept in code so the .tscn stays a single node) --------

func _build_ui() -> void:
	set_anchors_preset(Control.PRESET_FULL_RECT)
	var bg := ColorRect.new()
	bg.color = Color(0.09, 0.10, 0.13)
	bg.set_anchors_preset(Control.PRESET_FULL_RECT)
	add_child(bg)

	# The board, using the pixel-art asset as its backdrop.
	var board_rect := TextureRect.new()
	board_rect.texture = load("res://assets/board.png")
	board_rect.texture_filter = CanvasItem.TEXTURE_FILTER_NEAREST
	board_rect.stretch_mode = TextureRect.STRETCH_SCALE
	board_rect.custom_minimum_size = Vector2(BOARD_PX, BOARD_PX)
	board_rect.size = Vector2(BOARD_PX, BOARD_PX)
	board_rect.position = Vector2(24, 24)
	board_rect.mouse_filter = Control.MOUSE_FILTER_STOP
	board_rect.gui_input.connect(_on_board_input)
	add_child(board_rect)

	board_font = ThemeDB.get_fallback_font()

	# Highlights below, pieces on top, both drawn by hand so glyphs land dead-centre in
	# their squares (a Label centres on the font's line box, not the glyph, so chess
	# glyphs drift high and left).
	overlay = Node2D.new()
	overlay.draw.connect(_draw_overlay)
	board_rect.add_child(overlay)

	pieces_layer = Node2D.new()
	pieces_layer.draw.connect(_draw_pieces)
	board_rect.add_child(pieces_layer)

	# Side panel.
	var panel := VBoxContainer.new()
	panel.position = Vector2(BOARD_PX + 48, 24)
	panel.custom_minimum_size = Vector2(340, BOARD_PX)
	add_child(panel)

	var title := Label.new()
	title.text = "Stanchion Chess"
	title.add_theme_font_size_override("font_size", 26)
	panel.add_child(title)

	status_label = Label.new()
	status_label.add_theme_font_size_override("font_size", 18)
	panel.add_child(status_label)

	var diff_row := HBoxContainer.new()
	panel.add_child(diff_row)
	var diff_caption := Label.new()
	diff_caption.text = "Difficulty:"
	diff_row.add_child(diff_caption)
	for name in DIFFICULTY.keys():
		var button := Button.new()
		button.text = name
		button.toggle_mode = true
		button.button_pressed = name == difficulty
		button.pressed.connect(_on_difficulty.bind(name))
		diff_row.add_child(button)

	var buttons := HBoxContainer.new()
	panel.add_child(buttons)

	var restart := Button.new()
	restart.text = "New game"
	restart.pressed.connect(_on_restart)
	buttons.add_child(restart)

	# Re-reads the combination plugins from disk without restarting the game — edit a
	# tactic's Lua, press this, and the AI plays the new logic on its next move.
	var reload := Button.new()
	reload.text = "Reload combinations"
	reload.pressed.connect(_on_reload)
	buttons.add_child(reload)

	var ai_caption := Label.new()
	ai_caption.text = "\nBlack's reasoning"
	ai_caption.add_theme_font_size_override("font_size", 16)
	panel.add_child(ai_caption)

	ai_label = RichTextLabel.new()
	ai_label.bbcode_enabled = true
	ai_label.fit_content = true
	ai_label.custom_minimum_size = Vector2(340, 130)
	ai_label.text = "[i]Black moves after you do.[/i]"
	panel.add_child(ai_label)

	var hint_caption := Label.new()
	hint_caption.text = "\nCombinations available to you"
	hint_caption.add_theme_font_size_override("font_size", 16)
	panel.add_child(hint_caption)

	hint_label = RichTextLabel.new()
	hint_label.bbcode_enabled = true
	hint_label.fit_content = true
	hint_label.custom_minimum_size = Vector2(340, 200)
	panel.add_child(hint_label)

	var log_caption := Label.new()
	log_caption.text = "\nMoves"
	log_caption.add_theme_font_size_override("font_size", 16)
	panel.add_child(log_caption)

	log_label = RichTextLabel.new()
	log_label.fit_content = true
	log_label.custom_minimum_size = Vector2(340, 160)
	panel.add_child(log_label)


func _on_difficulty(name: String) -> void:
	difficulty = name
	_sync_difficulty_buttons()
	_refresh()


func _sync_difficulty_buttons() -> void:
	for node in _all_buttons(self):
		if DIFFICULTY.has(node.text):
			node.button_pressed = node.text == difficulty


func _all_buttons(node: Node) -> Array:
	var found: Array = []
	for child in node.get_children():
		if child is Button:
			found.append(child)
		found += _all_buttons(child)
	return found


func _on_restart() -> void:
	board = ChessBoard.new()
	selected = -1
	selected_moves = []
	game_over = ""
	move_log = []
	ai_last_move = {}
	if ai_label != null:
		ai_label.text = "[i]Black moves after you do.[/i]"
	_refresh()
	if not _human_turn():
		_ai_move()


## Hot-reloads the combination plugins from disk. Each already-loaded plugin is
## re-read (a broken edit leaves the old version running); a fresh scan picks up any
## newly added plugin folder. The player's hint refreshes immediately with the new
## logic, and the AI uses it on its next move.
func _on_reload() -> void:
	if stanchion == null:
		_flash_reload("Stanchion extension not loaded — nothing to reload.")
		return
	var reloaded := 0
	var failed: Array = []
	for name in stanchion.names():
		if stanchion.reload(name):
			reloaded += 1
		else:
			failed.append(str(name))
	# Pick up any plugin folder that was added since the game started.
	var report: Dictionary = stanchion.load("")
	var added: Array = report.get("loaded", [])

	var parts: Array = ["Reloaded %d combination(s)" % reloaded]
	if not added.is_empty():
		parts.append("added %s" % ", ".join(added))
	if not failed.is_empty():
		parts.append("kept old for %s (see log)" % ", ".join(failed))
	_flash_reload(", ".join(parts) + ".")
	_refresh()


func _flash_reload(message: String) -> void:
	_note(message)
	if ai_label != null:
		ai_label.text = "[color=#8cf][i]%s[/i][/color]" % message
