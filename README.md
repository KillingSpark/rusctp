# Rusctp

This is mostly a for-fun implementation of sctp. I am trying my hand at implementing stuff in a sans-io way but keeping useability in mind.

## Design goals

* The core must be sans-io
* The async wrapper should be runtime-agnostic
* The sync wrapper should somewhat resemble the usrsctp interface

## Can do
* Build a connection with another instance of itself and ping-pong packets

## Cant do / Roadmap
* (Not tested) build a connection with another implementation
* Shut down cleanly
* "Weird" association initialization (which is the normal case for webrtc)
* Congestion control reacting to loss
* Retransmission of lost packets
* Stream resets