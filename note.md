# how will gossip work?

## globally unique record ids

going ahead with the idea described here:
https://www.codyhiar.com/blog/reading-ddia-and-solving-gossip-glomers-in-python-part-2/

each node has a unique id (i configure this manually), as well as a atomic counter

each record has the composite key (node, count)

## push / pull

on a write we try and propagate the write to our neighbors. we do not retry in case of failure.

to reconcile differences in state, nodes occasionally request the state of their peers. how do we do so efficiently? this ties back into the global counter!

we can simply ask our neighbor for its count, and if its "clock" or counter is higher than when we previously adjusted it, we know that there are updates we need to poll. we then transit the full state.

likewise, upon receiving a gossip message, we can also verify the clock, and if it is higher than expected, we know to poll that particular node.

## conflict resolution

again, we can utilize the vector clock and see if one message came AFTER the other.

if we are unsure, we resort to a fallback of timestamps

# architecture

all services can call db
http and daemon can call control plane
