# Rusctp

This is mostly a for-fun implementation of sctp. I am trying my hand at implementing stuff in a sans-io way but keeping useability in mind.

Testing focuses on traffic wrapped in UDP but the sans-io core should not depend on that.

## Design goals

* The core must be sans-io
* The async wrapper should be runtime-agnostic
* The sync wrapper should somewhat resemble the usrsctp interface

## Can do
* Build a connection with another instance of itself and ping-pong packets
* Congestion control reacting to loss
* Retransmission of lost packets
* "Weird" association initialization (which is the normal case for webrtc)
* Shut down process
* PMTU detection/discovery: https://datatracker.ietf.org/doc/rfc8899/

## Cant do / Roadmap
* Reconfiguration (https://datatracker.ietf.org/doc/html/rfc6525)
* Parameterised reliability (https://www.rfc-editor.org/rfc/rfc3758.html)

# Interoperability
## Usersctp
* If rusctp is the server tscp can connect as a client and send data
* If tsctp is the server usrsctp can connect as a client and send data
* Usrsctp doesn't seem to make any pmtu probing, maybe that is just a setting missing in tsctp?
