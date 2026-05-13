"""Synthetic fixture that exercises every detector in src/insights.rs.

Designed to be small but contain ONE example of each finding kind so the
Insights viewer page renders a meaningful preview. Not production code —
this file intentionally contains antipatterns.
"""

from sqlalchemy.orm import Session
import requests
import logging

logger = logging.getLogger(__name__)


class OrderRepository:
    """Two methods that ought to be batched but aren't."""

    def __init__(self, session: Session):
        self.session = session

    def save_each(self, orders):
        # SMELL: n_plus_one AND noisy_log on the SAME symbol — refactor
        # candidate cluster (two findings on one node).
        for o in orders:
            logger.info("saving order %s", o)
            self.session.add(o)
            self.session.commit()
        return orders

    def find_each(self, ids):
        # SMELL: n_plus_one — db query in a loop
        out = []
        for i in ids:
            out.append(self.session.query("orders").filter_by(id=i).first())
        return out


async def sync_fetch_in_async(uid):
    # SMELL: blocking_in_async — requests.get is sync; not awaited
    return requests.get(f"https://api.example.com/users/{uid}")


def walk_tree(node):
    # SMELL: recursive — mutual recursion with descend_children below
    if node is None:
        return 0
    return 1 + descend_children(node)


def descend_children(node):
    # SMELL: recursive (paired with walk_tree)
    return sum(walk_tree(c) for c in node.children)


def process_batch(repo, orders):
    # Top-level entry — pulls in the repo + the async fn + the recursion.
    repo.save_each(orders)
    for o in orders:
        # not detected today, just a hot-path candidate
        logger.debug("processed order %s", o)
    return walk_tree(orders)
