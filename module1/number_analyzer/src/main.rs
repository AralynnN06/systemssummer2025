fn is_even(n: i32) -> bool {
    n % 2 == 0
}

fn main() {
    let numbers = [4, 15, 6, 9, 20, 13, 30, 11, 3, 10];

    // Check even/odd and FizzBuzz rules
    for &n in numbers.iter() {
        if n % 3 == 0 && n % 5 == 0 {
            println!("{n} => FizzBuzz");
        } else if n % 3 == 0 {
            println!("{n} => Fizz");
        } else if n % 5 == 0 {
            println!("{n} => Buzz");
        } else if is_even(n) {
            println!("{n} => Even");
        } else {
            println!("{n} => Odd");
        }
    }

    // While loop to find sum
    let mut sum = 0;
    let mut i = 0;
    while i < numbers.len() {
        sum += numbers[i];
        i += 1;
    }
    println!("\nSum of numbers: {sum}");

    // Find and print largest number
    let mut max = numbers[0];
    for &n in numbers.iter() {
        if n > max {
            max = n;
        }
    }
    println!("Largest number: {max}");
}