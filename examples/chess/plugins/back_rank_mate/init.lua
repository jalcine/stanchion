-- "Back Rank Mate": rook or queen mates on the victim's back rank, where the king
-- is boxed in by its own pawns. Reads is_mate + piece + destination rank.
local BackRankMate = {}
BackRankMate.__index = BackRankMate

function BackRankMate.new(config, deps)
  return setmetatable({}, BackRankMate)
end

function BackRankMate:name() return "Back Rank Mate" end

function BackRankMate:suggest(position)
  local back = 7
  if position.side == "b" then back = 0 end
  for _, move in ipairs(position.moves) do
    if move.is_mate and (move.piece == "R" or move.piece == "Q")
        and math.floor(move.to / 8) == back then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 900,
        name = "Back Rank Mate",
        rationale = move.san .. " mates on the back rank — no escape square.",
      }
    end
  end
  return nil
end

return BackRankMate
