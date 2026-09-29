-- "Queen fork": the queen lands where she attacks two valuable pieces. Queens
-- fork more often than knights do — and get forked in return just as easily,
-- so this only fires on two genuinely valuable targets.
local QueenFork = {}
QueenFork.__index = QueenFork

function QueenFork.new(config, deps)
  return setmetatable({}, QueenFork)
end

function QueenFork:name() return "Queen Fork" end

local NAMES = { P = "pawn", N = "knight", B = "bishop", R = "rook", Q = "queen", K = "king" }

function QueenFork:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.piece == "Q" and move.attacks_valuable >= 2 then
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
    name = "Queen Fork",
    rationale = "Queen to " .. best.to_sq .. " forks the " .. table.concat(hit, " and ") .. ".",
  }
end

return QueenFork
