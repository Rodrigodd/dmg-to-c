module nested_priority(
    input logic enable, reset_n, set, t0,
    output logic q
);
    initial q = 0;

    always_latch if (enable) begin
        if (!reset_n) q = 0;
        if (set && t0) q <= 1;
    end
endmodule
