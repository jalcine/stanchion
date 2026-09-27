-- "Knight fork": land a knight where it attacks two or more valuable pieces at once.
-- The classic tactic — and a good example of a combination that needs *derived* facts
-- about the position (what a piece attacks after moving), which Godot annotates onto
-- every move as `attacks` / `attacks_valuable`.
local KnightFork = {}
KnightFork.__index = KnightFork

function KnightFork.new(config, deps)
  return setmetatable({}, KnightFork)
end

function KnightFork:name() return "Knight Fork" end

local NAMES = { P = "pawn", N = "knight", B = "bishop", R = "rook", Q = "queen", K = "king" }

function KnightFork:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.piece == "N" and move.attacks_valuable >= 2 then
      if best == nil or move.attacks_valuable > best.attacks_valuable then
        best = move
      end
    end
  end
  if best == nil then return nil end
  local hit = {}
  for _, code in ipairs(best.attacks) do
    hit[#hit + 1] = NAMES[code] or code
  end
  return {
    from = best.from,
    to = best.to,
    promote = "",
    strength = 500 + best.attacks_valuable * 20,
    name = "Knight Fork",
    rationale = "Nf to " .. best.to_sq .. " forks the " .. table.concat(hit, " and ") .. ".",
  }
end

return KnightFork
