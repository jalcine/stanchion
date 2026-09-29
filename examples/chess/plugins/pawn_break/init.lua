-- "Pawn break": drive a pawn onto the sixth rank, cracking the position open.
-- Pawn breaks change every plan on the board — worth a nudge even unprompted.
local PawnBreak = {}
PawnBreak.__index = PawnBreak

function PawnBreak.new(config, deps)
  return setmetatable({}, PawnBreak)
end

function PawnBreak:name() return "Pawn Break" end

function PawnBreak:suggest(position)
  local sixth = 5
  if position.side == "b" then sixth = 2 end
  for _, move in ipairs(position.moves) do
    if move.piece == "P" and math.floor(move.to / 8) == sixth then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 220,
        name = "Pawn Break",
        rationale = move.san .. " breaks through to the sixth.",
      }
    end
  end
  return nil
end

return PawnBreak
