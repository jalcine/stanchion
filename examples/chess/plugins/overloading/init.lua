-- "Overloading": attack two things while taking a guarded one — the defender
-- gets a second job it cannot do. Reads attacks_valuable + captured_defended.
local Overloading = {}
Overloading.__index = Overloading

function Overloading.new(config, deps)
  return setmetatable({}, Overloading)
end

function Overloading:name() return "Overloading" end

function Overloading:suggest(position)
  local best = nil
  for _, move in ipairs(position.moves) do
    if move.attacks_valuable >= 2 and move.captured_defended then
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
    strength = 510 + best.attacks_valuable * 10,
    name = "Overloading",
    rationale = best.san .. " gives the defender two jobs and one move.",
  }
end

return Overloading
