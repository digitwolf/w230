# Working backwards: press release and FAQ

Written before the polish, to keep the project honest about who it is for
and what "done" means. If a change does not make one of the moments below
better, it is probably not worth doing.

## Press release

**Kawasaki W230 owners can now see their gear at a glance, without cutting a
wire or touching the ECU**

The W230 is a light, retro 233 cc single with a six-speed gearbox and a
cluster that shows no gear. Riders new to the bike stall it pulling away in
second, hunt for a seventh gear at 100 km/h, and lug it in sixth through
town; even experienced owners glance down at a cluster that cannot answer
the question. Kawasaki sells no indicator for it, and universal aftermarket
units need a button-pressing calibration ride per gear, splice into speed
and tach wires, and drift.

The W230 gear indicator is a matchbox-sized LED matrix that shows N and 1–6.
It plugs into the diagnostic connector the dealer uses, listens to the
engine speed and road speed the ECU already broadcasts, and infers the gear.
It reads only: nothing is written to the ECU, no factory wire is cut, and
it comes off in ten minutes. On the first ride the digits are already right
from Kawasaki's published gearing; after a minute in each gear it has learned
this particular bike, tyres and speedo error included.

The companion iPhone app answers the question every owner eventually asks,
"is it right, and if not, why?": it shows what the indicator sees, what it
has learned, and what went wrong, and it keeps the firmware current over
WiFi so the laptop is needed exactly once.

"I wanted the number the bike should have shipped with, and I wanted to
know I could trust it," says the builder. "Everything else followed from
that."

Parts cost about 50 USD; plans, firmware and app are open source.

## Customer FAQ

**Will it be right?** From the first ride the digits come from Kawasaki's
own gear ratios and are right at steady speed. It learns your bike as you
ride, then the digit locks in faster and stays right under acceleration.
While stopped, coasting with the clutch in, or launching with a slipping
clutch there is no true answer, so it shows a dash (or 1 for the launch).

**Can it damage the bike or void anything?** It only listens on the
diagnostic line, exactly like a dealer's tester does. The one wire that
needs care is the neutral-lamp wire, which is tapped through a diode. It
draws under 100 mA from a fused, key-switched circuit.

**Do I need to be an electronics person?** You need to solder or crimp
eight connections and follow a wiring picture. No programming: the firmware
is flashed once by running two commands, and updates after that come through
the app.

**What if it shows the wrong gear?** The app's Troubleshoot screen checks
the common causes (wiring, power, the diode, calibration state) and explains
what it found. Wipe-and-relearn takes one ride.

**Why a phone app at all?** Because "is it right?" deserves an answer you
can get in the driveway: live readings, the learned calibration, the ride
black box, and updates without a laptop.

**Does it work on other Kawasakis?** The protocol is shared; the register
decodings and gear ratios are W230-specific. Other models are a porting
project, not a setting.

## Internal FAQ

**Why infer the gear instead of reading it?** A full scan of the ECU's
diagnostic services found no gear or neutral register. rpm/speed is the
only signal, so the design invested in making that inference trustworthy:
factory-anchored bands, learning that refines only the gear it has
evidence for, debounce tuned on real rides.

**Why over-the-air updates for a hobby device?** The indicator lives on the
bike under a seat or behind a cluster. Every fix that needs a laptop and a
USB cable is a fix that does not get installed.

**What would make us stop?** If the digit could not be made right at cruise
on a real W230, or if the install required cutting factory wiring.
