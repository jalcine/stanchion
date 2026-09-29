-- "Back-rank threat": check on the back rank without mate — the mate threat
-- lingers after the check is parried, and the defence usually costs material.
local BackRankThreat = {}
BackRankThreat.__index = BackRankThreat

function BackRankThreat.new(config, deps)
  return setmetatable({}, BackRankThreat)
end

function BackRankThreat:name() return "Back-Rank Threat" end

function BackRankThreat:suggest(position)
  local back = 7
  if position.side == "b" then back = 0 end
  for _, move in ipairs(position.moves) do
    if move.gives_check and not move.is_mate
        and (move.piece == "R" or move.piece == "Q")
        and math.floor(move.to / 8) == back then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 440,
        name = "Back-Rank Threat",
        rationale = move.san .. " knocks on the back rank — mate hangs in the air.",
      }
    end
  end
  return nil
end

return BackRankThreat
