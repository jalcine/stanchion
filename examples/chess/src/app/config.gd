## Difficulty rosters: which combination plugins each level may consult. Fewer,
## softer combinations make a gentler opponent; the tactical ones make it bite.
extends RefCounted

const DIFFICULTY := {
	"Gentle": ["openings", "center_control", "develop_pieces", "pawn_promotion", "hanging_piece"],
	"Steady": ["openings", "center_control", "develop_pieces", "castle_safety", "win_material", "create_pin", "trade_winner", "hanging_piece", "pawn_promotion", "passed_pawn_push", "rook_open_file", "check_and_gain", "desperado", "king_flight", "back_rank_threat", "en_passant_theme", "avoid_trade_behind"],
	"Sharp": ["openings", "center_control", "develop_pieces", "castle_safety", "win_material", "create_pin", "knight_fork", "skewer", "discovered_check", "checkmate", "back_rank_mate", "smothered_mate", "arabian_mate", "anastasia_mate", "dovetail_mate", "bodens_mate", "hook_mate", "kill_box_mate", "pawn_fork", "bishop_fork", "rook_fork", "queen_fork", "royal_fork", "double_check", "check_and_gain", "cross_check", "removal_of_defender", "deflection", "decoy", "overloading", "interference", "x_ray_attack", "battery", "sacrifice", "hanging_piece", "trade_winner", "simplify_ahead", "avoid_trade_behind", "desperado", "pawn_promotion", "passed_pawn_push", "pawn_break", "knight_underpromotion", "rook_open_file", "rook_seventh", "outpost_knight", "activate_king", "back_rank_threat", "king_flight", "en_passant_theme"],
}

static func roster(difficulty: String) -> Array:
	return DIFFICULTY.get(difficulty, [])
