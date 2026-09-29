-- "Smothered Mate": a knight mates a king fenced in by its own pieces. Validates
-- the surround from position.board and stays silent otherwise, so the generic
-- Checkmate plugin (which never misnames) owns unvalidated mates.
local SmotheredMate = {}
SmotheredMate.__index = SmotheredMate

function SmotheredMate.new(config, deps)
  return setmetatable({}, SmotheredMate)
end

function SmotheredMate:name() return "Smothered Mate" end

-- Godot Arrays arrive 1-indexed; square 0 (a1) is board[1].
local function at(board, sq) return board[sq + 1] end

local STEPS = {{1, 0}, {1, 1}, {0, 1}, {-1, 1}, {-1, 0}, {-1, -1}, {0, -1}, {1, -1}}

local function surrounded(board, king, foe)
  local f = king % 8
  local r = math.floor(king / 8)
  for _, d in ipairs(STEPS) do
    local nf, nr = f + d[1], r + d[2]
    if nf >= 0 and nf < 8 and nr >= 0 and nr < 8 then
      local code = at(board, nr * 8 + nf)
      if code == "" or code:sub(1, 1) ~= foe then
        return false
      end
    end
  end
  return true
end

function SmotheredMate:suggest(position)
  local foe = "b"
  if position.side == "b" then foe = "w" end
  local king = -1
  for sq = 0, 63 do
    if at(position.board, sq) == foe .. "K" then
      king = sq
      break
    end
  end
  if king == -1 or not surrounded(position.board, king, foe) then
    return nil
  end
  for _, move in ipairs(position.moves) do
    if move.is_mate and move.piece == "N" then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 900,
        name = "Smothered Mate",
        rationale = move.san .. " smothers the fenced-in king.",
      }
    end
  end
  return nil
end

return SmotheredMate
