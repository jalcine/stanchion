-- "Simplify when ahead": up material, trade pieces not pawns — every swap drains
-- the opponent's counterplay while the lead stands. Reads position.material.
local SimplifyAhead = {}
SimplifyAhead.__index = SimplifyAhead

function SimplifyAhead.new(config, deps)
  return setmetatable({}, SimplifyAhead)
end

function SimplifyAhead:name() return "Simplify When Ahead" end

function SimplifyAhead:suggest(position)
  local mat = position.material or {}
  local foe = "b"
  if position.side == "b" then foe = "w" end
  if (mat[position.side] or 0) - (mat[foe] or 0) < 5 then
    return nil
  end
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.captured_value >= 3 and (move.mover_value or 0) >= 3 then
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
    strength = 260,
    name = "Simplify When Ahead",
    rationale = best.san .. " trades down with the lead intact.",
  }
end

return SimplifyAhead
