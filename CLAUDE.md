# CLAUDE.md - Branch Strategy for Dependency Management

## Library Dependencies
- **npm packages**: Listed in package.json
- **Python dependencies**: Specified in setup.cfg
- **Rust dependencies**: Defined in Cargo.toml

## Branch Strategy
1. **Branch Naming**:
   - `feat(dependency-update):<package-name>`
   - `fix(dependency-update):<package-version>`
2. **Commit Process**:
   - Run `npm outdated`, `pip list --outdated`, `cargo outdated` to identify updates
   - Use `git add -p` to selectively stage dependency changes
   - Commit with version number and vulnerability fixes cited
3. **Integration Testing**:
   - Run `npm test`, `pytest`, `cargo test` after updates
   - Validate cross-dependency compatibility in isolated Docker containers

## Repository Structure
```
adf45678 README.md
123abc CLAUDE.md
```
