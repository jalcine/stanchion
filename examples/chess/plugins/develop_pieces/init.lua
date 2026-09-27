-- "Develop your pieces": get the minor pieces off the back rank in the opening. Prefers
-- a move that also eyes an enemy piece, so development comes with a little pressure.
local Develop = {}
Develop.__index = Develop

function Develop.new(config, deps)
  return setmetatable({ until_move = config.until_move or 12 }, Develop)
end

function Develop:name() return "Develop Your Pieces" end

-- A back-rank square is rank 1 (white) or rank 8 (black) in algebraic notation.
local function on_back_rank(square)
  local rank = square:sub(2)
  return rank == "1" or rank == "8"
end

function Develop:suggest(position)
  if position.fullmove > self.until_move then return nil end
  local best = nil
  for _, move in ipairs(position.moves) do
    if (move.piece == "N" or move.piece == "B")
      and on_back_rank(move.from_sq) and not on_back_rank(move.to_sq) then
      -- A developing move that also attacks something is the pick of the bunch.
      local score = 90 + move.attacks_valuable * 10
      if best == nil or score > best.score then
        best = { move = move, score = score }
      end
    end
  end
  if best == nil then return nil end
  return {
    from = best.move.from,
    to = best.move.to,
    promote = "",
    strength = best.score,
    name = "Develop Your Pieces",
    rationale = "Develop the " .. (best.move.piece == "N" and "knight" or "bishop")
      .. " to " .. best.move.to_sq .. ".",
  }
end

return Develop
