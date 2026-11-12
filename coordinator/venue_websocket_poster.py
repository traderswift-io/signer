import asyncio
import json
import time
from urllib.parse import urlsplit

import aiohttp

from coordinator.account_holds import mark_account_held, settle_lost_nonce
from coordinator.venue_poster import LOOPBACK_HOSTS, post_signed_action

LIVE_HYPERLIQUID_WEBSOCKET_URL = 'wss://api.hyperliquid.xyz/ws'
WEBSOCKET_POST_DEADLINE_SECONDS = 3
WEBSOCKET_CONNECT_TIMEOUT_SECONDS = 5
KEEPALIVE_PING_SECONDS = 30


def refuse_venue_websocket_url(venue_websocket_url: str, allow_live_hyperliquid_exchange: bool) -> None:
    if venue_websocket_url == '':
        return
    if allow_live_hyperliquid_exchange is True and venue_websocket_url == LIVE_HYPERLIQUID_WEBSOCKET_URL:
        return
    if urlsplit(venue_websocket_url).scheme == 'ws' and urlsplit(venue_websocket_url).hostname in LOOPBACK_HOSTS:
        return
    raise ValueError(f'venue_websocket_url {venue_websocket_url} refused: empty, ws:// loopback, or exactly {LIVE_HYPERLIQUID_WEBSOCKET_URL} '
                     'with allow_live_hyperliquid_exchange true')


def build_websocket_state() -> dict:
    return {'socket': None, 'reader_task': None, 'keepalive_task': None, 'next_post_id': 1, 'waiting_by_post_id': {}, 'timed_out_posts': {}, 'round_state': None}


def fail_every_waiting_post(websocket_state: dict, reason: str) -> None:
    for waiting in websocket_state['waiting_by_post_id'].values():
        if not waiting.done():
            waiting.set_exception(ConnectionError(reason))
    websocket_state['waiting_by_post_id'].clear()


def settle_late_answer(websocket_state: dict, timed_out_post: dict, response: dict) -> None:
    venue_status, venue_answer = answer_from_websocket_response(response)
    round_state = websocket_state['round_state']
    released = round_state is not None and settle_lost_nonce(round_state, timed_out_post['vault_address'], timed_out_post['nonce'])
    print(json.dumps({'event': 'websocket_late_answer', 'source': 'signer-coordinator', 'nonce': str(timed_out_post['nonce']),
                      'vault_address': timed_out_post['vault_address'], 'venue_status': venue_status, 'hold_released': str(released),
                      'venue_answer': json.dumps(venue_answer)[:400]}), flush=True)


def resolve_waiting_post(websocket_state: dict, raw_text: str) -> None:
    try:
        envelope = json.loads(raw_text)
    except ValueError:
        return
    if not isinstance(envelope, dict) or envelope.get('channel') != 'post':
        return
    data = envelope.get('data') or {}
    if data.get('id') in websocket_state['timed_out_posts']:
        settle_late_answer(websocket_state, websocket_state['timed_out_posts'].pop(data.get('id')), data.get('response') or {})
        return
    waiting = websocket_state['waiting_by_post_id'].pop(data.get('id'), None)
    if waiting is not None and not waiting.done():
        waiting.set_result(data.get('response') or {})


async def read_websocket_answers(websocket_state: dict, socket: aiohttp.ClientWebSocketResponse) -> None:
    try:
        async for message in socket:
            if message.type == aiohttp.WSMsgType.TEXT:
                resolve_waiting_post(websocket_state, message.data)
    finally:
        if websocket_state['socket'] is socket:
            websocket_state['socket'] = None
        fail_every_waiting_post(websocket_state, 'websocket_closed')


async def keep_websocket_alive(socket: aiohttp.ClientWebSocketResponse) -> None:
    while not socket.closed:
        await asyncio.sleep(KEEPALIVE_PING_SECONDS)
        if not socket.closed:
            await socket.send_str(json.dumps({'method': 'ping'}))


async def open_websocket(http_session: aiohttp.ClientSession, websocket_state: dict, venue_websocket_url: str) -> aiohttp.ClientWebSocketResponse:
    socket = websocket_state['socket']
    if socket is not None and not socket.closed:
        return socket
    socket = await asyncio.wait_for(http_session.ws_connect(venue_websocket_url), WEBSOCKET_CONNECT_TIMEOUT_SECONDS)
    websocket_state['socket'] = socket
    websocket_state['reader_task'] = asyncio.create_task(read_websocket_answers(websocket_state, socket))
    websocket_state['keepalive_task'] = asyncio.create_task(keep_websocket_alive(socket))
    return socket


def answer_from_websocket_response(response: dict) -> tuple[str, dict]:
    if response.get('type') == 'action' and isinstance(response.get('payload'), dict):
        answer = response['payload']
        return ('ok' if answer.get('status') == 'ok' else 'err'), answer
    return 'err', {'status': 'err', 'response': response.get('payload'), 'websocket_response_type': response.get('type')}


async def post_over_websocket(http_session: aiohttp.ClientSession, websocket_state: dict, venue_websocket_url: str, body: dict) -> tuple[str, str, dict]:
    post_id = websocket_state['next_post_id']
    websocket_state['next_post_id'] += 1
    waiting = asyncio.get_running_loop().create_future()
    try:
        socket = await open_websocket(http_session, websocket_state, venue_websocket_url)
        websocket_state['waiting_by_post_id'][post_id] = waiting
        await socket.send_str(json.dumps({'method': 'post', 'id': post_id, 'request': {'type': 'action', 'payload': body}}))
    except (aiohttp.ClientError, ConnectionError, TimeoutError, asyncio.TimeoutError, RuntimeError) as cut:
        websocket_state['waiting_by_post_id'].pop(post_id, None)
        return 'not_sent', type(cut).__name__, {}
    try:
        response = await asyncio.wait_for(waiting, WEBSOCKET_POST_DEADLINE_SECONDS)
    except (ConnectionError, TimeoutError, asyncio.TimeoutError) as cut:
        websocket_state['timed_out_posts'][post_id] = {'vault_address': body.get('vaultAddress') or '', 'nonce': body.get('nonce')}
        return 'answer_lost', type(cut).__name__, {}
    finally:
        websocket_state['waiting_by_post_id'].pop(post_id, None)
    venue_status, venue_answer = answer_from_websocket_response(response)
    return 'answered', venue_status, venue_answer




async def post_signed_action_preferring_websocket(state: dict, body: dict) -> tuple[str, dict, dict]:
    config = state['config']
    timings = {}
    if config.venue_websocket_url:
        refuse_venue_websocket_url(config.venue_websocket_url, config.allow_live_hyperliquid_exchange)
        socket = state['websocket_state']['socket']
        timings['websocket_opened_for_this_post'] = socket is None or socket.closed
        started = time.perf_counter()
        outcome, detail, venue_answer = await post_over_websocket(state['http_session'], state['websocket_state'], config.venue_websocket_url, body)
        timings['websocket_ms'] = round((time.perf_counter() - started) * 1000, 1)
        if outcome == 'answered':
            timings['route'] = 'websocket'
            return detail, venue_answer, timings
        if outcome == 'answer_lost':
            timings['route'] = 'websocket_answer_lost'
            timings['websocket_failed'] = detail
            mark_account_held(state, body.get('vaultAddress') or '', body.get('nonce'), detail)
            return 'answer_lost', {'status': 'unknown', 'response': 'sent over websocket, no answer; order may be live; not resent'}, timings
        timings['websocket_failed'] = detail
    started = time.perf_counter()
    venue_status, venue_answer = await post_signed_action(state['http_session'], config.venue_exchange_url, config.allow_live_hyperliquid_exchange, body)
    timings['http_ms'] = round((time.perf_counter() - started) * 1000, 1)
    timings['route'] = 'http_fallback_not_sent_over_websocket' if 'websocket_failed' in timings else 'http'
    return venue_status, venue_answer, timings
