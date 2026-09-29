-- "Bishop fork": a bishop slides to a diagonal hitting two valuable pieces at
-- once — often a king and an unmoved rook. Reads attacks / attacks_valuable.
local BishopFork = {}
BishopFork.__index = BishopFork

function BishopFork.new(config, deps)
  return setmetatable({}, BishopFork)
end

function BishopFork:name() return "Bishop Fork" end

local NAMES = { P = "pawn", N = "knight", B = "bishop", R = "rook", Q = "queen", K = "king" }

function BishopFork:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.piece == "B" and move.attacks_valuable >= 2 then
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
    name = "Bishop Fork",
    rationale = "Bishop to " .. best.to_sq .. " forks the " .. table.concat(hit, " and ") .. ".",
  }
end

return BishopFork
