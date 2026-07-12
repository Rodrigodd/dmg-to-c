`default_nettype none

module direct_signal_bufif (
	input      logic in0, in1, ena0, ena1, ena2, t0,
	output tri logic y0, y1, y2
);

	bufif0 (y0, in0, ena0);
	bufif1 (y1, in1, ena1);
	bufif0 (strong1, highz0) (y2, in0, ena2 & t0);

endmodule

`default_nettype wire
