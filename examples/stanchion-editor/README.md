# Tauri Editor Example

This is an example editor built with [Tauri](https://tauri.app) using [SvelteKit](https://kit.svelte.dev) and [TypeScript](https://www.typescriptlang.org), with a Rust backend and Lua-powered editor extensions.

## Features

- **Tauri** desktop application framework
- **SvelteKit** for the frontend
- **TypeScript** for type safety
- **pnpm** for package management
- **Rust** backend via Tauri
- **Tiptap** rich text editor engine
- **Lua extensions** for custom editor logic

## Architecture

This example demonstrates:

1. **Tauri Backend**: Rust-based application with system integration capabilities
2. **SvelteKit Frontend**: Modern web framework with TypeScript support
3. **Tiptap Editor**: Powerful rich text editing engine
4. **Lua Extensions**: Custom editor functionality driven by Lua scripts

## Setup

### Prerequisites

- Node.js 18+ with pnpm
- Rust compiler (Cargo)
- Tauri CLI

### Installation

```bash
cd examples/new-example-editor
pnpm install
pnpm tauri android init
```

### Development

#### For Desktop Development:

```bash
pnpm tauri dev
```

#### For Android Development:

```bash
pnpm tauri android dev
```

## Current Status

This is the starter project for building an example editor with Tauri, SvelteKit, TypeScript, and Lua extensions. The editor is set up with:

- Basic Tauri structure
- SvelteKit with TypeScript
- Rust backend foundation
- Tiptap integration planned
- Lua extension framework ready

## Next Steps

1. **Add Tiptap**: Integrate Tiptap editor into the SvelteKit application
2. **Lua Extension System**: Set up Lua extension loading and execution
3. **Custom Extensions**: Build Lua-driven extensions for the editor
4. **Tauri API Integration**: Connect Rust backend with editor functionality
5. **Extension Architecture**: Design a system for loading and managing Lua extensions

## Project Structure

```
examples/new-example-editor/
├── src/                          # SvelteKit frontend
│   ├── lib/                     # Shared library
│   ├── routes/                  # SvelteKit routes
│   └── components/              # UI components
├── src-tauri/                    # Tauri Rust backend
│   ├── src/                     # Rust source code
│   ├── target/                  # Build artifacts
│   └── interfaces/              # Type definitions
├── static/                       # Static assets
├── plugins/                      # Extension plugins (Lua)
│   └── [extension-name]/        # Individual Lua extensions
│       ├── init.lua             # Extension logic
│       └── plugin.toml          # Extension metadata
└── package.json                 # pnpm project
```

## Lua Extensions

The editor extensions are driven by Lua scripts. Each extension is a sandboxed plugin:

```
plugins/
├── basic-text/
│   ├── init.lua              # Lua extension code
│   └── plugin.toml           # Extension metadata
├── advanced-formatting/
│   ├── init.lua
│   └── plugin.toml
└── table-editor/
    ├── init.lua
    └── plugin.toml
```

Each extension can:
- Modify editor behavior
- Add new commands
- Extend functionality
- Run in sandboxed environment

## Development Notes

### Tauri + SvelteKit Setup

This project uses the Tauri SvelteKit template with:

- `svelte.config.js`: SvelteKit configuration
- `vite.config.js`: Vite configuration for development
- `tsconfig.json`: TypeScript configuration
- `package.json`: pnpm dependencies

### Extension Architecture

The Lua extension system is inspired by the existing chess plugin system in this repository:

- **Sandboxed execution**: Each extension runs in an isolated environment
- **Rich metadata**: `plugin.toml` files define extension properties
- **Hot reload**: Extensions can be reloaded during development
- **System integration**: Extensions can access system capabilities via Tauri

### Type Safety

- Full TypeScript coverage
- Rust type definitions for Tauri interface
- Custom type definitions for Lua extensions
- Editor state typing

### Performance Considerations

- Extension sandboxing for security
- Efficient reloading mechanism
- Modular architecture for scalability
- Rust backend for performance-critical operations

## License

This is an example project for demonstration purposes.