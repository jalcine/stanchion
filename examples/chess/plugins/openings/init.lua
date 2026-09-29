-- Opening theory as a combination: follow a known book line while the game is still on
-- one. Each line is a SAN move sequence covering both sides, so this one plugin guides
-- White *and* Black into (and along) the opening. Once the game leaves every line, it
-- falls silent and the middlegame combinations take over.
--
-- Lines are the standard main lines, sourced from Wikipedia's opening articles:
--   Ruy Lopez / Morphy      https://en.wikipedia.org/wiki/Ruy_Lopez
--   Italian Game            https://en.wikipedia.org/wiki/Italian_Game
--   Sicilian, Najdorf       https://en.wikipedia.org/wiki/Sicilian_Defence,_Najdorf_Variation
--   Queen's Gambit Declined https://en.wikipedia.org/wiki/Queen%27s_Gambit_Declined
--   London System           https://en.wikipedia.org/wiki/London_System
--   French Defence          https://en.wikipedia.org/wiki/French_Defence
--   Caro-Kann Defence       https://en.wikipedia.org/wiki/Caro%E2%80%93Kann_Defence
local Openings = {}
Openings.__index = Openings

local BOOK = {
  { name = "Ruy Lopez", moves = { "e4", "e5", "Nf3", "Nc6", "Bb5", "a6", "Ba4", "Nf6", "O-O", "Be7" } },
  { name = "Italian Game", moves = { "e4", "e5", "Nf3", "Nc6", "Bc4", "Bc5", "c3", "Nf6", "d3" } },
  { name = "Sicilian, Najdorf", moves = { "e4", "c5", "Nf3", "d6", "d4", "cxd4", "Nxd4", "Nf6", "Nc3", "a6" } },
  { name = "Queen's Gambit Declined", moves = { "d4", "d5", "c4", "e6", "Nc3", "Nf6", "Bg5", "Be7" } },
  { name = "London System", moves = { "d4", "d5", "Bf4", "Nf6", "e3", "e6", "Nf3", "Bd6" } },
  { name = "French Defence", moves = { "e4", "e6", "d4", "d5", "Nc3", "Nf6" } },
  { name = "Caro-Kann Defence", moves = { "e4", "c6", "d4", "d5", "Nc3", "dxe4", "Nxe4" } },
}

function Openings.new(config, deps)
  return setmetatable({}, Openings)
end

function Openings:name() return "Openings" end

-- Does `line` begin with exactly the moves in `history`?
local function follows(history, line)
  if #line <= #history then return false end
  for i = 1, #history do
    if line[i] ~= history[i] then return false end
  end
  return true
end

function Openings:suggest(position)
  local history = position.history
  for _, entry in ipairs(BOOK) do
    if follows(history, entry.moves) then
      local want = entry.moves[#history + 1]
      for _, move in ipairs(position.moves) do
        if move.san == want then
          return {
            from = move.from,
            to = move.to,
            promote = move.promotes,
            -- While a game is still in book, theory leads: stronger than the positional
            -- principles and than speculative tactics, but below a concrete win of
            -- material, so the engine still grabs a hanging piece if one appears.
            strength = 300,
            name = entry.name,
            rationale = "Book move " .. want .. " in the " .. entry.name .. ".",
          }
        end
      end
    end
  end
  return nil
end

return Openings
