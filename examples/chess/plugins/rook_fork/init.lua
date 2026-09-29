-- "Rook fork": a rook takes a file or rank that hits two valuable pieces — the
-- classic seventh-rank double hit. Reads attacks / attacks_valuable.
local RookFork = {}
RookFork.__index = RookFork

function RookFork.new(config, deps)
  return setmetatable({}, RookFork)
end

function RookFork:name() return "Rook Fork" end

local NAMES = { P = "pawn", N = "knight", B = "bishop", R = "rook", Q = "queen", K = "king" }

function RookFork:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.piece == "R" and move.attacks_valuable >= 2 then
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
    name = "Rook Fork",
    rationale = "Rook to " .. best.to_sq .. " hits the " .. table.concat(hit, " and ") .. ".",
  }
end

return RookFork
