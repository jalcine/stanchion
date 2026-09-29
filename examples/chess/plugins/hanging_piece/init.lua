-- "Hanging piece": take what is free. An undefended capture is not a decision —
-- it outranks contested grabs, so it sits above Win Material on the same prey.
local HangingPiece = {}
HangingPiece.__index = HangingPiece

function HangingPiece.new(config, deps)
  return setmetatable({}, HangingPiece)
end

function HangingPiece:name() return "Hanging Piece" end

local NAMES = { P = "pawn", N = "knight", B = "bishop", R = "rook", Q = "queen", K = "king" }

function HangingPiece:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.captured_value >= 1 and not move.captured_defended then
      if best == nil or move.captured_value > best.captured_value then
        best = move
      end
    end
  end
  if best == nil then return nil end
  return {
    from = best.from,
    to = best.to,
    promote = best.promotes,
    strength = 340 + best.captured_value * 15,
    name = "Hanging Piece",
    rationale = "The " .. (NAMES[best.capture] or "piece") .. " on " .. best.to_sq .. " is hanging — take it.",
  }
end

return HangingPiece
