-- "Pawn fork": a pawn advances to attack two valuable pieces at once. The cheapest
-- fork on the board — reads attacks / attacks_valuable like the knight fork does.
local PawnFork = {}
PawnFork.__index = PawnFork

function PawnFork.new(config, deps)
  return setmetatable({}, PawnFork)
end

function PawnFork:name() return "Pawn Fork" end

local NAMES = { P = "pawn", N = "knight", B = "bishop", R = "rook", Q = "queen", K = "king" }

function PawnFork:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.piece == "P" and move.attacks_valuable >= 2 then
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
    promote = best.promotes,
    strength = 480 + best.attacks_valuable * 20,
    name = "Pawn Fork",
    rationale = "Pawn to " .. best.to_sq .. " forks the " .. table.concat(hit, " and ") .. ".",
  }
end

return PawnFork
