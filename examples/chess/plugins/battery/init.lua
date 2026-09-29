-- "Battery": line two sliders up on one ray — doubled rooks, queen behind rook.
-- The front piece threatens, the back one reloads. Scans rays from the
-- destination for an own slider directly behind the mover.
local Battery = {}
Battery.__index = Battery

function Battery.new(config, deps)
  return setmetatable({}, Battery)
end

function Battery:name() return "Battery" end

local function at(board, sq) return board[sq + 1] end

local ROOK_DIRS = {{1, 0}, {-1, 0}, {0, 1}, {0, -1}}
local BISHOP_DIRS = {{1, 1}, {1, -1}, {-1, 1}, {-1, -1}}

local function behind(board, to, side)
  local f = to % 8
  local r = math.floor(to / 8)
  local dirs = {}
  for _, d in ipairs(ROOK_DIRS) do dirs[#dirs + 1] = d end
  for _, d in ipairs(BISHOP_DIRS) do dirs[#dirs + 1] = d end
  for _, d in ipairs(dirs) do
    -- Look backwards from the destination: the ray the mover just vacated.
    local nf, nr = f - d[1], r - d[2]
    if nf >= 0 and nf < 8 and nr >= 0 and nr < 8 then
      local code = at(board, nr * 8 + nf)
      if code ~= "" and code:sub(1, 1) == side then
        local t = code:sub(2, 2)
        local straight = (d[1] == 0 or d[2] == 0)
        if t == "Q" or (t == "R" and straight) or (t == "B" and not straight) then
          return true
        end
      end
    end
  end
  return false
end

function Battery:suggest(position)
  for _, move in ipairs(position.moves) do
    if (move.piece == "R" or move.piece == "Q" or move.piece == "B")
        and (move.gives_check or move.attacks_valuable >= 1)
        and behind(position.board, move.to, position.side) then
      return {
        from = move.from,
        to = move.to,
        promote = move.promotes,
        strength = 470,
        name = "Battery",
        rationale = move.san .. " doubles the heavy pieces on one line.",
      }
    end
  end
  return nil
end

return Battery
