module timing_single_path (
    input logic a, b,
    output logic y
);
    assign y = a & b;

    specify
        (a, b *> y) = (T_single, T_fall, T_off);
    endspecify
endmodule
