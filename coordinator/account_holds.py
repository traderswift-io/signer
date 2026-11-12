import json
import time
from pathlib import Path

OBSERVED_STATES = ('filled', 'resting', 'cancelled', 'not_found')


def load_holds(hold_state_file: str) -> dict:
    if not hold_state_file or not Path(hold_state_file).exists():
        return {}
    return json.loads(Path(hold_state_file).read_text())


def save_holds(hold_state_file: str, holds: dict) -> None:
    if not hold_state_file:
        return
    temporary = Path(hold_state_file + '.tmp')
    temporary.write_text(json.dumps(holds, indent=1, sort_keys=True))
    temporary.chmod(0o600)
    temporary.replace(hold_state_file)


def hold_for(state: dict, vault_address: str) -> dict | None:
    return state['account_holds'].get((vault_address or '').lower())


def mark_account_held(state: dict, vault_address: str, nonce: int, reason: str) -> None:
    hold = state['account_holds'].setdefault((vault_address or '').lower(), {'lost_nonces': {}, 'since_ms': int(time.time() * 1000)})
    hold['lost_nonces'][str(nonce)] = reason
    save_holds(state['config'].hold_state_file, state['account_holds'])


def settle_lost_nonce(state: dict, vault_address: str, nonce: int) -> bool:
    hold = hold_for(state, vault_address)
    if hold is None or hold['lost_nonces'].pop(str(nonce), None) is None:
        return False
    if not hold['lost_nonces']:
        state['account_holds'].pop((vault_address or '').lower(), None)
    save_holds(state['config'].hold_state_file, state['account_holds'])
    return hold_for(state, vault_address) is None


def release_account_hold(state: dict, vault_address: str, observed_state: str, posted_by: str) -> dict | None:
    released = state['account_holds'].pop((vault_address or '').lower(), None)
    if released is not None:
        save_holds(state['config'].hold_state_file, state['account_holds'])
    return released
