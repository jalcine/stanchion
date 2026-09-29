-- "Dovetail Mate": the queen mates an edge king hemmed in by the dovetail of its
-- own pieces. Fires on queen mates against a king on any edge square.
local DovetailMate = {}
DovetailMate.__index = DovetailMate

function DovetailMate.new(config, deps)
  return setmetatable({}, DovetailMate)
end

function DovetailMate:name() return "Dovetail Mate" end

local function at(board, sq) return board[sq + 1] end

function DovetailMate:suggest(position)
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
    if move.is_mate and move.piece == "Q" then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 900,
        name = "Dovetail Mate",
        rationale = move.san .. " dovetails the queen into the pinned edge king.",
      }
    end
  end
  return nil
end

return DovetailMate
