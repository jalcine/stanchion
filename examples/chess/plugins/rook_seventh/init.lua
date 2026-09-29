-- "Seventh rank": a rook on the seventh eats pawns and ties the king down.
-- Pigs on the seventh keep the king boxed and the pawns pluckable.
local RookSeventh = {}
RookSeventh.__index = RookSeventh

function RookSeventh.new(config, deps)
  return setmetatable({}, RookSeventh)
end

function RookSeventh:name() return "Seventh Rank" end

function RookSeventh:suggest(position)
  local seventh = 6
  if position.side == "b" then seventh = 1 end
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.piece == "R" and math.floor(move.to / 8) == seventh
        and move.attacks_valuable >= 1 then
      if best == nil or move.attacks_valuable > best.attacks_valuable then
        best = move
      end
    end
  end
  if best == nil then return nil end
  return {
    from = best.from,
    to = best.to,
    promote = best.promotes,
    strength = 280 + best.attacks_valuable * 10,
    name = "Seventh Rank",
    rationale = best.san .. " — the rook feasts on the seventh.",
  }
end

return RookSeventh
