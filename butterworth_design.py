#!/usr/bin/env python3

"""Design and plot a digital lowpass Butterworth filter.

Uses SciPy's `scipy.signal.butter` to design the filter and Matplotlib to plot the
frequency response.

Example:
  ./butterworth_design.py --order 6 --cutoff-hz 20000 --fs-hz 192000
"""

from __future__ import annotations

import argparse
import sys

import numpy as np


def _parse_args(argv: list[str]) -> argparse.Namespace:
	parser = argparse.ArgumentParser(
		description="Design and plot a digital lowpass Butterworth filter."
	)
	parser.add_argument(
		"--order",
		"-n",
		type=int,
		required=True,
		help="Filter order (positive integer).",
	)
	parser.add_argument(
		"--cutoff-hz",
		"-c",
		type=float,
		required=True,
		help="Lowpass cutoff frequency in Hz.",
	)
	parser.add_argument(
		"--fs-hz",
		"-f",
		type=float,
		required=True,
		help="Sampling frequency in Hz.",
	)
	parser.add_argument(
		"--worN",
		type=int,
		default=4096,
		help="Number of frequency points for the response plot (default: 4096).",
	)
	parser.add_argument(
		"--sos",
		action="store_true",
		help="Design as second-order sections (recommended for higher orders).",
	)
	parser.add_argument(
		"--no-coeffs",
		action="store_true",
		help="Do not print filter coefficients to stdout.",
	)
	parser.add_argument(
		"--coeff-precision",
		type=int,
		default=16,
		help="Decimal digits for coefficient printing (default: 16).",
	)
	return parser.parse_args(argv)


def design_lowpass_butterworth(
	order: int, cutoff_hz: float, fs_hz: float, *, sos: bool
):
	if order <= 0:
		raise ValueError("order must be a positive integer")
	if fs_hz <= 0:
		raise ValueError("fs_hz must be > 0")
	if cutoff_hz <= 0:
		raise ValueError("cutoff_hz must be > 0")
	nyquist = fs_hz / 2.0
	if cutoff_hz >= nyquist:
		raise ValueError(f"cutoff_hz must be < Nyquist ({nyquist:g} Hz)")

	from scipy.signal import butter

	if sos:
		return butter(order, cutoff_hz, btype="lowpass", fs=fs_hz, output="sos")
	b, a = butter(order, cutoff_hz, btype="lowpass", fs=fs_hz, output="ba")
	return b, a


def plot_frequency_response(
	filt, *, order: int, fs_hz: float, cutoff_hz: float, worN: int, sos: bool
) -> None:
	import matplotlib.pyplot as plt
	from scipy.signal import freqz, sosfreqz

	if worN < 16:
		raise ValueError("worN must be >= 16")

	if sos:
		w_hz, h = sosfreqz(filt, worN=worN, fs=fs_hz)
	else:
		b, a = filt
		w_hz, h = freqz(b, a, worN=worN, fs=fs_hz)

	mag_db = 20.0 * np.log10(np.maximum(np.abs(h), 1e-12))
	phase = np.unwrap(np.angle(h))

	fig, (ax_mag, ax_phase) = plt.subplots(2, 1, sharex=True, figsize=(9, 6))
	fig.suptitle(
		f"Digital Butterworth Lowpass (order={order}, fc={cutoff_hz:g} Hz, fs={fs_hz:g} Hz)"
	)

	ax_mag.plot(w_hz, mag_db, lw=1.5)
	ax_mag.axvline(cutoff_hz, color="C1", ls="--", lw=1, label="cutoff")
	ax_mag.set_ylabel("Magnitude (dB)")
	ax_mag.grid(True, which="both", ls=":")
	ax_mag.legend(loc="best")

	ax_phase.plot(w_hz, phase, lw=1.0)
	ax_phase.axvline(cutoff_hz, color="C1", ls="--", lw=1)
	ax_phase.set_xlabel("Frequency (Hz)")
	ax_phase.set_ylabel("Phase (rad)")
	ax_phase.grid(True, which="both", ls=":")

	plt.tight_layout()
	plt.show()


def print_coefficients(filt, *, sos: bool, precision: int) -> None:
	if precision < 1:
		raise ValueError("coeff precision must be >= 1")

	floatmode = "maxprec_equal"

	if sos:
		sos_arr = np.asarray(filt, dtype=float)
		print("sos =")
		print(
			np.array2string(
				sos_arr,
				separator=", ",
				precision=precision,
				floatmode=floatmode,
				suppress_small=False,
			),
		)
		return

	b, a = filt
	print("b =")
	print(
		np.array2string(
			np.asarray(b, dtype=float),
			separator=", ",
			precision=precision,
			floatmode=floatmode,
			suppress_small=False,
		),
	)
	print("a =")
	print(
		np.array2string(
			np.asarray(a, dtype=float),
			separator=", ",
			precision=precision,
			floatmode=floatmode,
			suppress_small=False,
		),
	)


def main(argv: list[str]) -> int:
	args = _parse_args(argv)
	try:
		filt = design_lowpass_butterworth(
			order=args.order,
			cutoff_hz=args.cutoff_hz,
			fs_hz=args.fs_hz,
			sos=bool(args.sos),
		)
		if not bool(args.no_coeffs):
			print_coefficients(
				filt,
				sos=bool(args.sos),
				precision=int(args.coeff_precision),
			)
		plot_frequency_response(
			filt,
			order=args.order,
			fs_hz=args.fs_hz,
			cutoff_hz=args.cutoff_hz,
			worN=args.worN,
			sos=bool(args.sos),
		)
		return 0
	except Exception as e:
		print(f"error: {e}", file=sys.stderr)
		return 2


if __name__ == "__main__":
	raise SystemExit(main(sys.argv[1:]))
