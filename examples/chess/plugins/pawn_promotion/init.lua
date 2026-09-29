-- "Promotion": a pawn reaches the end and queens (or underpromotes). The single
-- biggest material swing on the board — outranks ordinary captures. Reads the
-- engine's promotes annotation.
local PawnPromotion = {}
PawnPromotion.__index = PawnPromotion

function PawnPromotion.new(config, deps)
  return setmetatable({}, PawnPromotion)
end

function PawnPromotion:name() return "Promotion" end

function PawnPromotion:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.promotes ~= "" then
      local gain = move.captured_value * 15
      if move.promotes == "Q" then gain = gain + 40 else gain = gain + 10 end
      if best == nil or gain > best.gain then
        best = { move = move, gain = gain }
      end
    end
  end
  if best == nil then return nil end
  return {
    from = best.move.from,
    to = best.move.to,
    promote = best.move.promotes,
    strength = 450 + best.gain,
    name = "Promotion",
    rationale = best.move.san .. " makes a new queen.",
  }
end

return PawnPromotion
