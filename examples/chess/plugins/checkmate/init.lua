-- "Mate in one": the strongest combination there is. If any legal move delivers
-- checkmate, nothing else is worth considering.
--
-- A combination plugin only ever *reads* the position Godot hands it and returns a
-- move to play. It never computes the rules of chess — every move in `position.moves`
-- already arrives annotated with what it does.
local Checkmate = {}
Checkmate.__index = Checkmate

function Checkmate.new(config, deps)
  return setmetatable({}, Checkmate)
end

function Checkmate:name() return "Checkmate" end

function Checkmate:suggest(position)
  for _, move in ipairs(position.moves) do
    if move.is_mate then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 1000,
        name = "Checkmate",
        rationale = move.piece .. move.to_sq .. " is mate.",
      }
    end
  end
  return nil
end

return Checkmate
