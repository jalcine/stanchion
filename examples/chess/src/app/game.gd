## Chess, where the *rules* live in the board components and the *plays* live in Lua.
##
## Game state and turn flow. Renders through BoardView, talks to plugins through
## StanchionClient, chooses moves through Brain, configures through Config.
## The combination catalogue does double duty: the AI picks the strongest applicable
## combination (and which combinations it is *allowed* to see sets the difficulty),
## while the player is shown every combination available to them as a hint.
extends Control

const ChessBoard = preload("res://src/board/board.gd")
const BoardView = preload("res://src/app/board_view.gd")
const SidePanel = preload("res://src/app/side_panel.gd")
const StanchionClient = preload("res://src/app/stanchion_client.gd")
const Brain = preload("res://src/app/brain.gd")
const Config = preload("res://src/app/config.gd")

var client = StanchionClient.new()
var board := ChessBoard.new()
var human := "w"
var difficulty := "Steady"

var selected := -1
var selected_moves: Array = []  # legal moves from the selected square
var hint_move: Dictionary = {}  # the top combination suggestion for the human
var ai_last_move: Dictionary = {}  # from/to of Black's last move, tinted while shown
var game_over := ""

var view: BoardView
var panel: SidePanel
var move_log: Array[String] = []


func _ready() -> void:
	set_anchors_preset(Control.PRESET_FULL_RECT)
	var bg := ColorRect.new()
	bg.color = Color(0.09, 0.10, 0.13)
	bg.set_anchors_preset(Control.PRESET_FULL_RECT)
	add_child(bg)

	view = BoardView.new()
	view.setup(self)
	add_child(view)
	view.square_picked.connect(_on_square)

	panel = SidePanel.new()
	panel.setup(BoardView.BOARD_PX, difficulty, Config.DIFFICULTY.keys())
	add_child(panel)
	panel.difficulty_chosen.connect(_on_difficulty)
	panel.restart_requested.connect(_on_restart)
	panel.reload_requested.connect(_on_reload)

	var err: String = client.open_engine(ProjectSettings.globalize_path("res://plugins"))
	if err != "":
		_note(err)
		return
	var loaded: Array = client.load_all()
	if loaded.is_empty():
		_note("No combination plugins loaded.")
	else:
		_note("Loaded combinations: " + ", ".join(loaded))
	_refresh()


# ---- turn flow --------------------------------------------------------------


func _human_turn() -> bool:
	return board.side_to_move == human and game_over == ""


func _on_square(sq: int) -> void:
	if not _human_turn():
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
	view.refresh()


func _try_human_move(from: int, to: int) -> void:
	for move in board.legal_moves(human):
		if move["from"] == from and move["to"] == to:
			# Auto-queen: prefer the promotion-to-queen variant when one exists.
			if move.get("promote", "") != "" and move["promote"] != "Q":
				continue
			_commit(move)
			return


func _commit(move: Dictionary) -> void:
	move_log.append(Brain.describe(board, move))
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
	var picks := client.suggest(board.position_for(color), Config.roster(difficulty))
	var chosen := Brain.match_suggestion(board, picks, color)
	if chosen.is_empty():
		chosen = Brain.fallback_move(board, color)
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
	if panel.ai_label == null:
		return
	if chosen.is_empty():
		panel.ai_label.text = "[i]No legal move.[/i]"
		return
	var move_text := Brain.describe(board, chosen)
	var text := ""
	if chosen_pick.is_empty():
		text = (
			"[b]%s[/b]\n[color=#c9a][i]No combination applied — improvised.[/i][/color]\n"
			% move_text
		)
	else:
		text = (
			"[b]%s — %s[/b]\n%s\n"
			% [move_text, chosen_pick.get("name", ""), chosen_pick.get("rationale", "")]
		)
	# Show the combinations it considered but passed over, at this difficulty.
	if picks.size() > 1:
		text += "\n[color=#888]Also weighed:[/color]\n"
		for i in range(1, mini(picks.size(), 4)):
			text += (
				"[color=#888]· %s (%d)[/color]\n"
				% [picks[i].get("name", ""), int(picks[i].get("strength", 0))]
			)
	_note("AI played: " + move_text)
	panel.ai_label.text = text


func _check_end() -> void:
	var color := board.side_to_move
	if board.legal_moves(color).is_empty():
		if board.in_check(color):
			var winner := "White" if color == "b" else "Black"
			game_over = winner + " wins by checkmate"
		else:
			game_over = "Draw by stalemate"


# ---- rendering --------------------------------------------------------------


func _refresh() -> void:
	# Refresh the human's available combinations as a hint.
	hint_move = {}
	var hint_text := ""
	if _human_turn():
		# Show them everything.
		var picks := client.suggest(board.position_for(human), Config.roster("Sharp"))
		if not picks.is_empty():
			hint_move = picks[0]
			for pick in picks:
				hint_text += "[b]%s[/b]  %s\n" % [pick.get("name", ""), pick.get("rationale", "")]
		else:
			hint_text = "[i]No named combination here — trust a principle.[/i]"
	panel.hint_label.text = hint_text

	if game_over != "":
		panel.status_label.text = game_over
	elif _human_turn():
		panel.status_label.text = "Your move (White)"
	else:
		panel.status_label.text = "Black is thinking…"

	panel.log_label.text = ""
	var n := move_log.size()
	var start := maxi(0, n - 16)
	for i in range(start, n):
		panel.log_label.text += "%d. %s\n" % [i + 1, move_log[i]]

	view.refresh()


func _note(text: String) -> void:
	print("[chess] ", text)
	if panel != null and panel.status_label != null:
		panel.status_label.tooltip_text = text


# ---- panel intents ----------------------------------------------------------


func _on_difficulty(roster_name: String) -> void:
	difficulty = roster_name
	panel.sync_difficulty(difficulty, Config.DIFFICULTY)
	_refresh()


func _on_restart() -> void:
	board = ChessBoard.new()
	selected = -1
	selected_moves = []
	game_over = ""
	move_log = []
	ai_last_move = {}
	if panel.ai_label != null:
		panel.ai_label.text = "[i]Black moves after you do.[/i]"
	_refresh()
	if not _human_turn():
		_ai_move()


## Hot-reloads the combination plugins from disk. Each already-loaded plugin is
## re-read (a broken edit leaves the old version running); a fresh scan picks up any
## newly added plugin folder. The player's hint refreshes immediately with the new
## logic, and the AI uses it on its next move.
func _on_reload() -> void:
	if not client.is_live():
		_flash_reload("Stanchion extension not loaded — nothing to reload.")
		return
	var reloaded := 0
	var failed: Array = []
	for plugin_name in client.plugin_names():
		if client.reload_plugin(plugin_name):
			reloaded += 1
		else:
			failed.append(str(plugin_name))
	# Pick up any plugin folder that was added since the game started.
	var added: Array = client.load_all()

	var parts: Array = ["Reloaded %d combination(s)" % reloaded]
	if not added.is_empty():
		parts.append("added %s" % ", ".join(added))
	if not failed.is_empty():
		parts.append("kept old for %s (see log)" % ", ".join(failed))
	_flash_reload(", ".join(parts) + ".")
	_refresh()


func _flash_reload(message: String) -> void:
	_note(message)
	if panel.ai_label != null:
		panel.ai_label.text = "[color=#8cf][i]%s[/i][/color]" % message
