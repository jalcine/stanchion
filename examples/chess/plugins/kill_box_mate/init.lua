-- "Kill Box Mate": any mate against a king driven to the edge with no flight
-- square. The catch-all named pattern — the specific ones (back rank, Arabian…)
-- claim their shapes first by sitting at the same strength; this one only needs
-- the edge king.
local KillBoxMate = {}
KillBoxMate.__index = KillBoxMate

function KillBoxMate.new(config, deps)
  return setmetatable({}, KillBoxMate)
end

function KillBoxMate:name() return "Kill Box Mate" end

local function at(board, sq) return board[sq + 1] end

function KillBoxMate:suggest(position)
  local foe = "b"
  if position.side == "b" then foe = "w" end
  local king = -1
  for sq = 0, 63 do
    if at(position.board, sq) == foe .. "K" then
      king = sq
      break
    end
  end
  if king == -1 then return nil end
  local f = king % 8
  local r = math.floor(king / 8)
  if not (f == 0 or f == 7 or r == 0 or r == 7) then
    return nil
  end
  for _, move in ipairs(position.moves) do
    if move.is_mate then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 900,
        name = "Kill Box Mate",
        rationale = move.san .. " boxes the edge king in — mate.",
      }
    end
  end
  return nil
end

return KillBoxMate
