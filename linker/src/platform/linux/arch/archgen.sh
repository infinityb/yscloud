while read line
do
    syscall_nr=$(printf "SYS_$line" | gcc -include sys/syscall.h -E - | tail -1)
    printf "pub const /*%04d*/ SYSCALL_NR_%s: usize = %d;\n" $syscall_nr $(echo ${line} | tr a-z A-Z) $syscall_nr
done <syscalls.txt | sort -u