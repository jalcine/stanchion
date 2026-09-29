## Square math and piece-code helpers. Leaf module — no dependencies.
extends RefCounted


static func file_of(sq: int) -> int:
	return sq % 8


static func rank_of(sq: int) -> int:
	@warning_ignore("integer_division")
	return sq / 8


static func in_board(file: int, rank: int) -> bool:
	return file >= 0 and file < 8 and rank >= 0 and rank < 8


static func square_name(sq: int) -> String:
	return "abcdefgh"[sq % 8] + str(int(sq / 8) + 1)


static func color_of(code: String) -> String:
	return code.substr(0, 1) if code != "" else ""


static func type_of(code: String) -> String:
	return code.substr(1, 1) if code != "" else ""


static func opponent(color: String) -> String:
	return "b" if color == "w" else "w"
