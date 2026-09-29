-- "Hook Mate": rook and knight mate an edge king propped up by its own pawn —
-- the pawn is the hook the mate hangs on. Validates the pawn wedge by scan.
local HookMate = {}
HookMate.__index = HookMate

function HookMate.new(config, deps)
  return setmetatable({}, HookMate)
end

function HookMate:name() return "Hook Mate" end

local function at(board, sq) return board[sq + 1] end

local STEPS = {{1, 0}, {1, 1}, {0, 1}, {-1, 1}, {-1, 0}, {-1, -1}, {0, -1}, {1, -1}}

function HookMate:suggest(position)
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
  local wedge = false
  for _, d in ipairs(STEPS) do
    local nf, nr = f + d[1], r + d[2]
    if nf >= 0 and nf < 8 and nr >= 0 and nr < 8 then
      if at(position.board, nr * 8 + nf) == foe .. "P" then
        wedge = true
        break
      end
    end
  end
  if not wedge then return nil end
  for _, move in ipairs(position.moves) do
    if move.is_mate and (move.piece == "N" or move.piece == "R") then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 900,
        name = "Hook Mate",
        rationale = move.san .. " hooks the mate onto the king's own pawn.",
      }
    end
  end
  return nil
end

return HookMate
