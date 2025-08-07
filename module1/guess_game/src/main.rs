fn check_guess(guess: i32, secret: i32) -> i32 {
    if guess == secret {
        0
    } else if guess > secret {
        1
    } else {
        -1
    }
}

fn main() {
    let secret = 7;
    let guesses = [3, 5, 8, 7]; // simulated input
    let mut attempts = 0;

    for guess in guesses {
        attempts += 1;
        match check_guess(guess, secret) {
            0 => {
                println!("Guess {guess} is correct! 🎉");
                break;
            }
            1 => println!("Guess {guess} is too high."),
            -1 => println!("Guess {guess} is too low."),
            _ => {}
        }
    }

    println!("Total guesses: {attempts}");
}