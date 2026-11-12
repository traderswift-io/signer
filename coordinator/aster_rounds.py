import uuid

from coordinator.coordinator_rounds import (MILLISECONDS_PER_SECOND, MINIMUM_REARM_AHEAD_SECONDS, close_out_deadline_milliseconds, is_new_risk,
                                            log_event, now_milliseconds, prepare_round, raise_nonce_floor_from_refusal,
                                            resolve_venue_address)
from coordinator.signer_client import ask_signers
from coordinator.venue_digest import aster_message_digest, aster_signed_query, recover_address

ASTER_VENUE = 'aster'


def aster_countdown_time_or_none(state: dict, policy: dict) -> int | None:
    wanted = state['config'].dead_man_window_seconds * MILLISECONDS_PER_SECOND
    left_before_deadline = close_out_deadline_milliseconds(policy) - now_milliseconds()
    countdown_time = min(wanted, left_before_deadline)
    return countdown_time if countdown_time >= MINIMUM_REARM_AHEAD_SECONDS * MILLISECONDS_PER_SECOND else None


def build_aster_rearm_intent(state: dict, delegate_key: str, vault_address: str, policy_epoch: int, symbol: str,
                             countdown_time: int, venue_nonce: int) -> dict:
    config = state['config']
    return {
        'request_id': uuid.uuid4().hex,
        'key_ref': config.key_ref,
        'delegate_key': delegate_key,
        'policy_epoch': policy_epoch,
        'venue': ASTER_VENUE,
        'vault_address': vault_address,
        'kind': 'countdown_cancel_all',
        'symbol': symbol,
        'countdown_time': countdown_time,
        'venue_nonce': venue_nonce,
        'expires_at': now_milliseconds() // MILLISECONDS_PER_SECOND + config.intent_lifetime_seconds,
    }


async def prepare_aster_round(state: dict, delegate_key: str, vault_address: str, symbol: str | None) -> dict:
    prepared = await prepare_round(state, delegate_key, vault_address)
    if not prepared.get('prepared'):
        return prepared
    policy = state['policy_by_delegate'][delegate_key]
    countdown_time = aster_countdown_time_or_none(state, policy) if symbol else None
    if countdown_time is None:
        return {**prepared, 'rearm_intent': None}
    caller_nonce = max(prepared['venue_nonce'] + 1, state['highest_issued_nonce'] + 1)
    state['highest_issued_nonce'] = caller_nonce
    rearm_intent = build_aster_rearm_intent(state, delegate_key, vault_address, prepared['policy_epoch'], symbol, countdown_time,
                                            prepared['venue_nonce'])
    return {**prepared, 'venue_nonce': caller_nonce, 'rearm_intent': rearm_intent}


def aster_reply_refusal(intent: dict, reply: dict, venue_address: str) -> str | None:
    try:
        method, path, signed_query = aster_signed_query(intent, venue_address, reply.get('nonce'))
    except (KeyError, ValueError, TypeError):
        return 'aster_query_is_not_the_expected_one'
    if (signed_query, method, path) != (reply.get('signed_query'), reply.get('method'), reply.get('path')):
        return 'aster_query_is_not_the_expected_one'
    digest = aster_message_digest(signed_query)
    if digest.hex() != reply.get('digest'):
        return 'digest_is_not_the_expected_one'
    if recover_address(digest, reply['r'], reply['s'], reply['v']) != venue_address:
        return 'signature_does_not_recover_to_venue_address'
    if reply.get('signature') != f"0x{reply['r']}{reply['s']}{reply['v']:02x}":
        return 'signature_text_is_not_r_s_v'
    return None


async def run_aster_signing_round(state: dict, intent: dict) -> dict:
    request_id = intent.get('request_id', 'unnamed')
    venue_address = await resolve_venue_address(state)
    if venue_address is None:
        return {'request_id': request_id, 'signed': False, 'reason': 'venue_address_unknown'}
    reply = await ask_signers(state['http_session'], state['config'].signer_urls, intent)
    if not reply.get('signed'):
        reason = reply.get('reason', 'no_reason')
        if not reason.startswith('aster_nonce'):
            raise_nonce_floor_from_refusal(state, reason)
        log_event('aster_round_refused', request_id, kind=intent.get('kind'), reason=reason)
        return {'request_id': request_id, 'signed': False, 'reason': reason}
    refusal = aster_reply_refusal(intent, reply, venue_address)
    if refusal is not None:
        log_event('aster_round_mismatch', request_id, reason=refusal)
        return {'request_id': request_id, 'signed': False, 'reason': refusal}
    log_event('aster_round_signed', request_id, kind=intent['kind'], nonce=reply['nonce'], asked=reply.get('asked'))
    return {'request_id': request_id, 'signed': True, 'signed_query': reply['signed_query'], 'signature': reply['signature'],
            'nonce': reply['nonce'], 'method': reply['method'], 'path': reply['path'], 'digest': reply['digest'],
            'recovered': venue_address, 'asked': reply.get('asked')}


async def handle_aster_intent(state: dict, intent: dict, rearm_intent: dict | None) -> dict:
    rearm_outcome = await run_aster_signing_round(state, rearm_intent) if rearm_intent is not None else None
    if is_new_risk(intent) and not (rearm_outcome or {}).get('signed'):
        reason = rearm_outcome.get('reason', 'rearm_not_signed') if rearm_outcome else 'rearm_required'
        log_event('aster_new_risk_stopped_no_dead_man', intent.get('request_id', 'unnamed'), reason=reason)
        return {'request_id': intent.get('request_id'), 'signed': False, 'reason': f'dead_man_not_armed: {reason}', 'rearm': rearm_outcome}
    outcome = await run_aster_signing_round(state, intent)
    outcome['rearm'] = rearm_outcome
    return outcome
