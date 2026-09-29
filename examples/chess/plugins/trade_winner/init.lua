-- "Winning trade": swap a cheap piece for an expensive one, guard or no guard.
-- Mover value vs captured value is now engine-annotated, so the math is exact.
local TradeWinner = {}
TradeWinner.__index = TradeWinner

function TradeWinner.new(config, deps)
  return setmetatable({}, TradeWinner)
end

function TradeWinner:name() return "Winning Trade" end

function TradeWinner:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    local diff = move.captured_value - (move.mover_value or 0)
    if move.captured_value > 0 and diff > 0 then
      if best == nil or diff > best.diff then
        best = { move = move, diff = diff }
      end
    end
  end
  if best == nil then return nil end
  return {
    from = best.move.from,
    to = best.move.to,
    promote = best.move.promotes,
    strength = 330 + best.diff * 15,
    name = "Winning Trade",
    rationale = best.move.san .. " wins the exchange (+" .. best.diff .. ").",
  }
end

return TradeWinner
