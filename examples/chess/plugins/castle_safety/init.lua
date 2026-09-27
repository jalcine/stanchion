-- "Castle early": tuck the king away. A positional combination rather than a tactic —
-- it never wins material, but a safe king is what lets the others work.
local CastleSafety = {}
CastleSafety.__index = CastleSafety

function CastleSafety.new(config, deps)
  return setmetatable({}, CastleSafety)
end

function CastleSafety:name() return "Castle for Safety" end

function CastleSafety:suggest(position)
  for _, move in ipairs(position.moves) do
    if move.is_castle then
      return {
        from = move.from,
        to = move.to,
        promote = "",
        strength = 200,
        name = "Castle for Safety",
        rationale = "Castle to " .. move.to_sq .. " and get the king off the centre.",
      }
    end
  end
  return nil
end

return CastleSafety
